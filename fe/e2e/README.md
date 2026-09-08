# Explorer browser checks

Run against a running Solvent frontend and API on a seeded devnet with at least one confirmed trade
in the DAI/USDC pool (the starter seeds this pair).
Google Chrome must be installed; Playwright launches a fresh profile. These checks read existing
records and do not require a connected wallet.

From the repository root:

```sh
pnpm --dir fe test:e2e
```

Defaults are frontend `http://localhost:5173` and API `http://localhost:8080`. Set `SOLVENT_FE_URL`
and `SOLVENT_API_URL` to test another deployment; the frontend must use that same API.

The list check exercises available cursor pages, status filtering, and activity pagination. The
detail check opens a confirmed trade directly, verifies its recorded lifecycle and configured
explorer links, reloads, and returns to the live list. Expected records come from the API response,
so the checks tolerate different trade IDs, counts, and a final empty cursor page.

The navigation check opens a pool settlement through its real trade link, verifies the transition
retains the outgoing view until the sweep covers it, and exercises browser Back/Forward. Its paused
browser clock makes the animation boundaries deterministic without submitting another trade.

Screenshots go to `fe/test-results/`; failed runs also retain a Playwright trace. They are local
artifacts, not screenshot snapshots used as assertions. Type checking includes the E2E source;
Vitest excludes it so browser checks run only through `test:e2e`.
