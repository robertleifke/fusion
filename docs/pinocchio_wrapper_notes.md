# PR2 Pinocchio Wrapper Notes

This documents the explicit validation and dispatch behavior implemented in
`program/src/lib.rs` for the Anchor -> Pinocchio wrapper migration.

## Dispatcher

- Manual instruction dispatch using Anchor global discriminators:
  - `initialize_market`: `2323bdc19b30aacb`
  - `place_order`: `33c29baf6d82606a`
  - `cancel_order`: `5f81edf00831df84`
  - `claim_order_proceeds`: `feb0c820781c5c34`

## Account layout compatibility

- `Market` account still uses the same 8-byte discriminator and Borsh field order.
- Discriminator preserved as `dbbed53700e3c69a` (`account:Market`).
- `OrderNode` and orderbook logic are unchanged from PR1 extraction.

## Validation model

Per-instruction validators now explicitly enforce:
- signer/writable flags
- token program identity (`Tokenkeg...` or `Tokenz...`)
- PDA derivation checks for `market` and `vault_authority`
- market has_one relationships (mints/vaults)
- token account mint/owner pairings
- duplicate mutable account rejection

## Token-2022 screening

- Mints with extensions are currently rejected (`len > 82`) to avoid
  extension-dependent transfer semantics during wrapper migration.
- Token accounts may still have extension bytes; base fields are validated.

## CPI path

- `anchor_spl::transfer_checked` replaced by explicit
  `pinocchio_token_2022::instructions::TransferChecked` CPIs.
- PDA signer seeds are explicitly constructed and passed to `invoke_signed`.

## Current initialize behavior

- The wrapper now recreates Anchor-like init semantics:
  - system `create_account` CPI for `market` (PDA signer) and token vaults
  - token `initialize_account3` CPI for vault initialization
  - rent-based lamports/space calculation from sysvar
  - explicit signer/writable/owner checks before CPI

## Added negative-path coverage

- malformed instruction data
- unknown discriminator
- duplicate mutable accounts
- missing signer
- readonly where writable is required
- wrong token program
- wrong vault authority PDA
- bad market discriminator/layout bytes
- wrong mint/token-account pairing
