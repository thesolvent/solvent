# Contributing to Solvent

Thanks for your interest. Solvent is a zero-inventory intent resolver on 1inch Aqua; the on-chain code
lives in `contracts/` (Foundry) and the design in `docs/`.

## Development setup

```sh
just setup     # yarn install (1inch npm deps) + forge install (Uniswap submodules)
just build
just test
git config core.hooksPath .githooks   # enable fmt + comment-standard pre-commit hook
```

`just` is optional — every recipe in the [`justfile`](justfile) is a plain command you can run by hand.

## Workflow

1. **Branch** off `main` — `feat/…`, `fix/…`, `docs/…`, or `chore/…`.
2. **Keep production code interface-only** where it integrates external protocols — Solvent is a *taker*;
   it never re-implements Aqua/SwapVM/UniswapX.
3. **Tests earn their place** — test logic that can regress, not config/getters/struct-init.
4. **Run the gate** before pushing: `just gate` (fmt-check + build + test).
5. **Open a PR** into `main`; CI (fmt/build/test) must be green. Fill in the PR template.

## Conventions

- Solidity `0.8.30`, `forge fmt` enforced.
- Comments explain **why**, not what; most lines need none.
- Commits: imperative mood, scoped subject (e.g. `contracts: add reactorCallback guard`).
- Dependencies: 1inch Aqua/SwapVM via npm (`package.json` + `yarn.lock`); Uniswap via git submodule
  (`foundry.lock`). Pin everything; no floating versions, no external paths.

## Reporting bugs / security

Functional bugs → open an issue. **Security vulnerabilities → do not open a public issue**; see
[`SECURITY.md`](SECURITY.md).

By contributing you agree to the [Code of Conduct](CODE_OF_CONDUCT.md) and that your contributions are
licensed under the [MIT License](LICENSE).
