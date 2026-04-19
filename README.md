# Fusion

Fully onchain, no-crank orderbook on Solana.

This repository contains an Anchor MVP that executes matching and settlement
inside `place_order` with no offchain keeper loops.

The repo layout is now aligned with the monorepo pattern used in
`Ellipsis-Labs/plasma`:
- `program/` for the onchain program
- `crates/*` for shared Rust crates
- `docs/` for ABI and migration specs
- `idl/` for generated interface files
- `sdk/` for offchain clients
- `audits/` for security artifacts

## What is implemented

- Single-market central limit orderbook in one account (`Market`)
- Price-time priority linked lists (bids and asks)
- Synchronous matching on `place_order`
- Immediate taker settlement in the same transaction
- Maker proceeds accrual and explicit `claim_order_proceeds`
- `cancel_order` for resting orders
- Fully collateralized resting orders through SPL Token vaults

## Architecture (MVP)

- Program: `program/src/lib.rs`
- Shared primitives + canonical instruction codec: `crates/fusion_engine/src/lib.rs`
- Market keeps a fixed-capacity order array (`MAX_ORDERS`)
- Bids/asks are linked lists by index for deterministic ordering
- No external crank:
  - Every `place_order` call performs matching directly
  - Vault transfers for taker outputs happen immediately

## Repository layout

```text
.
├── audits/
├── docs/
├── crates/
│   ├── fusion_engine/
│   └── fusion_state/
├── idl/
├── program/
│   └── src/lib.rs
├── sdk/
├── Anchor.toml
├── Cargo.toml
└── start-test-validator
```

## Important current constraints

- Fixed max active order slots per market (`MAX_ORDERS = 128`)
- Amounts are raw token units (no lot conversion layer yet)
- `claim_order_proceeds` currently uses the owner signer model
- One market account design for simple deterministic behavior

## ABI freeze for wrapper swap

To support the Anchor->Pinocchio wrapper migration, the current Anchor ABI
surface is frozen in:

- `docs/anchor_abi_spec.md`

## Next steps

1. Add lot-size and tick-size enforcement.
2. Add per-user open-order indexing for faster UX queries.
3. Add market-level fee model and fee vaults.
4. Add integration tests with realistic matching scenarios.
5. Add optional consume-events style path for high-throughput variants while
   preserving synchronous settlement for takers.
