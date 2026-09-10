# Decay rounding, cosignature verification, and why admission is an allow-list

Written alongside Phase U task U1. Three ideas, each of which cost real money to get wrong
somewhere else before it was written down.

---

## 1. Rounding the distance, and which way each branch rounds

A Dutch auction interpolates an amount between two points in time. The obvious implementation is a
lerp:

```
amount(t) = start + (end − start) · (t − t₀) / (t₁ − t₀)
```

With integers you must round somewhere, and the natural instinct is to round the *result*. That
instinct is what the original `AmountCurve` encoded: a falling curve floored the answer, a rising
curve ceiled it, on the theory that both should land "in the swapper's favour."

`DutchDecayLib` rounds the **distance travelled** instead, and — this is the part worth burning in —
the two branches round *opposite ways*:

```solidity
if (endAmount < startAmount) {
    decayedAmount = startAmount - (startAmount - endAmount).mulDivDown(elapsed, duration);
} else {
    decayedAmount = startAmount + (endAmount - startAmount).mulDivUp(elapsed, duration);
}
```

- **Falling** — a floored distance subtracted from `start` leaves a *larger* remainder.
- **Rising** — a ceiled distance added to `start` gives a *larger* total.

Both move the amount against the filler, which is the invariant. Rounding the distance the *same*
way in both directions is one wei out on a rising curve:

```
0 → 1000 over [0, 3], at t = 1
  ceil the distance:   0 + ceil(1000/3)  = 334   ← the contract
  floor the distance:  0 + floor(1000/3) = 333   ← one wei under
```

On UniswapX a rising curve is the *input* side of an exact-output order — the reactor reverts
`IncorrectAmounts` if an output rises — so this affects what we model ourselves as receiving, on the
~6.6% of live orders with a decaying input.

## The correction, and how it happened

**This article originally said the opposite** — that both branches use `mulDivDown` and a rising
curve should floor. That was wrong, and the code was changed to match it, replacing correct
behaviour with a one-wei error.

The source of the mistake: upstream `main` has since refactored `decay` into a signed `linearDecay`
helper, and *that* version does floor both branches. It is not the code we fill against. The
deployed V2 reactor, and the `contracts/lib/UniswapX` submodule this repo pins — the same code the
fill E2E source-deploys — both use `mulDivUp` when rising. Verified by building the pinned library
and calling it: `0 → 1000 over [0,3]` at `t=1` returns **334**.

**The lesson, sharpened.** The original article already said the right thing — *"a test that encodes
your own model cannot detect that your model is wrong; only an independent oracle can"* — and then
the replacement test transcribed the **wrong version** of the contract into `u128` and swept 180
points against it. A hand-written transcription is not an independent oracle; it is the same model
written twice. It looked like the strongest evidence in the change and was defending the bug.

An oracle is only independent if it comes from outside your own understanding: the deployed
bytecode, the pinned submodule executed under `forge`, or values pinned from either. `mainnet_fork_parity.rs`
does this correctly for SwapVM pricing — the expected amounts are what the deployed router returned.
The same discipline was not applied to the one piece of arithmetic this change actually touched.

And the version question is the other half: "port the operations, not the intent" is necessary but
not sufficient. You have to port the operations *of the deployment you are talking to*, and pin the
evidence to it.

---

## 2. Cosignature verification, and what the reactor does *not* check

UniswapX V2 orders carry two signatures with different shapes.

**The swapper's** is a Permit2 EIP-712 witness over the *base* order — everything except
`cosignerData`. That exclusion is the whole design: it lets a third party improve the order's terms
after the user has signed, without invalidating their signature.

**The cosigner's** covers the mutable part:

```solidity
ecrecover(keccak256(abi.encodePacked(orderHash, abi.encode(order.cosignerData))), v, r, s)
```

Note `abi.encodePacked` on the outside and `abi.encode` on the inner struct — mixing those up
produces a digest that verifies against nothing.

The subtle part is what the reactor does with the recovered address:

```solidity
if (order.cosigner != signer || signer == address(0)) revert InvalidCosignature();
```

It compares the signer to **a field in the order itself**. It has no notion of a legitimate
cosigner. Any keypair can cosign an order that names it.

This is not a flaw — `cosigner` is inside the swapper's signed hash, so the *swapper* chose who may
parameterize their order, and the reactor is enforcing exactly that choice. But it has two
consequences for a filler:

1. **Pinning the cosigner identity is our job.** An order can be perfectly valid on-chain and still
   be one we want nothing to do with. Hence `expected_cosigners` in the normalizer — an allow-list
   we own, checked *after* recovery, because recovering tells you who signed and the allow-list
   tells you whether you care.
2. **A test harness can mint indistinguishable orders.** Because there is no privileged key, an
   order our own builder cosigns is structurally identical to a Uniswap-cosigned one; the only
   difference is the address in `expected_cosigners`. That is what lets the fork harness be faithful
   rather than a simulation.

A related trap: the order hash is computed **before** cosigner overrides are applied — the reactor
comments this explicitly, because it is the hash the user signed. Compute it after and every
signature check fails.

---

## 3. Admission is an allow-list, because the input is adversarial

Until now, orders entered the pipeline from our own venue: a taker posted a signed order to
`POST /v1/swap`, and we cosigned it. The order's shape was, in effect, ours.

Reading from a public API inverts that. Anyone can post an order naming any token pair, any
recipient, any validation hook, and our filler as the exclusive filler. Nothing in the protocol
prevents it, and nothing should — it is a permissionless system.

The instinct is a deny-list: block the tokens known to misbehave. That fails for a structural
reason. The set of ways an ERC20 can violate the assumption "transferring N moves exactly N" is
open-ended — fee-on-transfer, rebasing, blocklists, pausable transfers, hooks that reenter,
approve-to-zero reverts — and each new one is discovered by losing gas to it.

The allow-list works because of an economic accident: **we can only fill orders whose tokens have
Aqua maker positions.** That set is small, known, and already required. So restricting to it costs
literally nothing in reach, and it collapses the entire class of hostile-token griefing into a
lookup. A constraint that was already true becomes a security boundary for free.

The other admission rules follow the same shape — each rejects an order we would otherwise pay gas
to discover we cannot settle:

- **Native-currency legs.** The reactor pays a native output from *its own balance*
  (`recipient.call{value:}`), which requires the filler to forward `msg.value`. Our filler is
  ERC20-only. ~1/3 of live mainnet orders have a native output, so this is a third of the flow we
  must decline cleanly rather than attempt and revert on.
- **`additionalValidationContract != 0`.** An arbitrary contract the reactor calls inside our fill,
  with our address as an argument. It can burn unbounded gas, or revert only on state an `eth_call`
  cannot reproduce — which turns our simulation into a false positive and hands an attacker a
  repeatable "sim passes, chain reverts" oracle.
- **Cosigner override bounds.** The reactor reverts if an override worsens the swapper's terms.
  Checking it costs two comparisons; discovering it on-chain costs a transaction.
- **An output-count cap.** Each output is a token to source and a term in the filler's quadratic
  output scan.
- **A bounded dedup cache.** TTL alone is not a bound when an adversary controls the arrival rate;
  the allow-list check runs *before* the cache insert so junk cannot consume slots.

The unifying principle: on an adversarial input boundary, enumerate what you can handle and reject
everything else. The failure mode of a deny-list is unbounded; the failure mode of an allow-list is
a missed opportunity, which is recoverable and visible.

---

## Where the "guard" is not a guard

`UniswapXAquaFiller` snapshots every token the plan touches and asserts non-decreasing balances at
the end of the fill. It is a genuine safety property: a hostile token, a hostile router, or an
under-sourcing plan reverts the whole transaction and we lose nothing.

It is tempting to treat that as covering the ingest question. It does not, and the distinction is
worth holding precisely:

| Covered | Not covered |
|---|---|
| Theft of the filler's balances | Gas spent on a reverting fill |
| Paying a maker more than planned | Zero-margin fills (non-decreasing ≠ profitable) |
| An output the plan did not source | The order counting as a missed fill |

So a hostile order cannot rob us — it can only make us waste gas, repeatedly, at no cost to
itself. That converts a solvency problem into a griefing problem, which is the right trade, but a
griefing problem is still a problem you solve at the door rather than at the vault.
