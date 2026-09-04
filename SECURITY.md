# Security Policy

Solvent is pre-1.0, unaudited research software built for ETHOnline 2026. **Do not use it with real
funds on mainnet.**

## Reporting a vulnerability

Please **do not open a public issue** for security vulnerabilities.

Instead, use GitHub's private ["Report a vulnerability"](https://github.com/21r21a33333/solvent/security/advisories/new)
flow (Security → Advisories), or email the maintainer at **void.00.diwakar@gmail.com** with:

- a description of the issue and its impact,
- steps to reproduce (a failing test or PoC is ideal),
- any suggested remediation.

We aim to acknowledge reports within a few days. Coordinated disclosure is appreciated — please give us a
reasonable window to fix before any public write-up.

## Scope

In scope: the on-chain contracts under `contracts/src/`. Out of scope: third-party dependencies
(1inch Aqua/SwapVM, Uniswap UniswapX/permit2, OpenZeppelin) — report those upstream.
