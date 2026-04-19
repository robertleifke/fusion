# Fusion Anchor ABI Freeze (for Pinocchio wrapper migration)

This document freezes the current Anchor account ABI and invariant checks from
`program/src/lib.rs` so PR2 can reproduce behavior exactly.

## Program

- Program ID: `Fus1on1111111111111111111111111111111111111`

## Instruction: `initialize_market(market_bump: u8, vault_authority_bump: u8)`

### Accounts (in order)

1. `admin` - signer, writable
2. `market` - writable, PDA `seeds=["market", admin]`, bump=`market_bump`
3. `base_mint` - read
4. `quote_mint` - read
5. `base_vault` - writable token account (mint=`base_mint`, authority=`vault_authority`)
6. `quote_vault` - writable token account (mint=`quote_mint`, authority=`vault_authority`)
7. `vault_authority` - read PDA `seeds=["vault_auth", market]`, bump=`vault_authority_bump`
8. `token_program` - read
9. `system_program` - read

### Runtime checks

- `market` PDA matches `create_program_address(["market", admin, market_bump])`.
- `vault_authority` matches `find_program_address(["vault_auth", market])`.
- `vault_authority_bump` equals derived bump.
- Initializes order free-list from `0..MAX_ORDERS-1` with tail `NONE_INDEX`.

## Instruction: `place_order(side: u8, limit_price: u64, quantity: u64)`

### Accounts (in order)

1. `user` - signer, writable
2. `market` - writable (`has_one` base/quote mint + base/quote vault)
3. `base_mint` - read
4. `quote_mint` - read
5. `base_vault` - writable
6. `quote_vault` - writable
7. `user_base_ata` - writable token account, owner=`user`, mint=`base_mint`
8. `user_quote_ata` - writable token account, owner=`user`, mint=`quote_mint`
9. `vault_authority` - read
10. `token_program` - read

### Runtime checks

- `side in {0,1}`.
- `limit_price > 0`, `quantity > 0`.
- `vault_authority == find_program_address(["vault_auth", market]).0`.
- Bid flow deposits `limit_price * quantity` quote before matching.
- Ask flow deposits `quantity` base before matching.
- Matching uses strict price crossing and price-time linked-list ordering.
- Unfilled remainder becomes resting order with locked collateral.
- Fully filled taker gets immediate unused collateral refund.

## Instruction: `cancel_order(order_id: u64)`

### Accounts (in order)

1. `owner` - signer
2. `market` - writable

### Runtime checks

- `order_id` must exist.
- `order.owner == owner`.
- `order.open == true`.
- If in book, remove from linked list; mark `open=false`.

## Instruction: `claim_order_proceeds(order_id: u64)`

### Accounts (in order)

1. `owner` - signer, writable
2. `market` - writable (`has_one` base/quote mint + base/quote vault)
3. `base_mint` - read
4. `quote_mint` - read
5. `base_vault` - writable
6. `quote_vault` - writable
7. `owner_base_ata` - writable token account, owner=`owner`, mint=`base_mint`
8. `owner_quote_ata` - writable token account, owner=`owner`, mint=`quote_mint`
9. `vault_authority` - read
10. `token_program` - read

### Runtime checks

- `order_id` must exist.
- `order.owner == owner`.
- `vault_authority == find_program_address(["vault_auth", market]).0`.
- `base_out = base_claimable + (!open ? locked_base : 0)`.
- `quote_out = quote_claimable + (!open ? locked_quote : 0)`.
- Transfer nonzero outputs from vaults to owner ATAs via `vault_authority` signer.
- If order is closed and claimables drained, free slot back to free-list.

## Serialization/layout assumptions to preserve in PR2

- Account discriminator remains Anchor 8-byte discriminator prefix.
- `Market` field order and widths remain unchanged.
- `OrderNode` field order and widths remain unchanged.
- `MAX_ORDERS=128`, `NONE_INDEX=-1`.
