use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};
use fusion_engine::{OrderNode, Side, MAX_ORDERS, NONE_INDEX};

declare_id!("Fus1on1111111111111111111111111111111111111");

const MARKET_SPACE: usize = 8 + 256 + (MAX_ORDERS * OrderNode::SIZE);

#[program]
pub mod fusion {
    use super::*;

    pub fn initialize_market(
        ctx: Context<InitializeMarketCtx>,
        market_bump: u8,
        vault_authority_bump: u8,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;

        let expected_market = Pubkey::create_program_address(
            &[b"market", ctx.accounts.admin.key().as_ref(), &[market_bump]],
            ctx.program_id,
        )
        .map_err(|_| error!(FusionError::InvalidPda))?;
        require_keys_eq!(expected_market, market.key(), FusionError::InvalidPda);

        let (expected_vault_authority, derived_bump) =
            Pubkey::find_program_address(&[b"vault_auth", market.key().as_ref()], ctx.program_id);
        require_keys_eq!(
            expected_vault_authority,
            ctx.accounts.vault_authority.key(),
            FusionError::InvalidPda
        );
        require_eq!(vault_authority_bump, derived_bump, FusionError::InvalidPda);

        market.admin = ctx.accounts.admin.key();
        market.base_mint = ctx.accounts.base_mint.key();
        market.quote_mint = ctx.accounts.quote_mint.key();
        market.base_vault = ctx.accounts.base_vault.key();
        market.quote_vault = ctx.accounts.quote_vault.key();
        market.bump = market_bump;
        market.vault_authority_bump = vault_authority_bump;
        market.bids_head = NONE_INDEX;
        market.asks_head = NONE_INDEX;
        market.free_head = 0;
        market.next_order_id = 1;
        market.order_count = 0;

        for i in 0..MAX_ORDERS {
            let next = if i + 1 < MAX_ORDERS {
                (i as i16) + 1
            } else {
                NONE_INDEX
            };
            market.orders[i] = OrderNode {
                next,
                ..OrderNode::default()
            };
        }

        Ok(())
    }

    pub fn place_order(
        ctx: Context<PlaceOrderCtx>,
        side: u8,
        limit_price: u64,
        quantity: u64,
    ) -> Result<()> {
        require!(side == Side::Bid as u8 || side == Side::Ask as u8, FusionError::InvalidSide);
        require_gt!(limit_price, 0, FusionError::InvalidPrice);
        require_gt!(quantity, 0, FusionError::InvalidQuantity);
        let mut quantity = quantity;

        let market = &mut ctx.accounts.market;
        let side = Side::try_from_u8(side).map_err(map_engine_error)?;
        let (expected_vault_authority, _) =
            Pubkey::find_program_address(&[b"vault_auth", market.key().as_ref()], ctx.program_id);
        require_keys_eq!(
            expected_vault_authority,
            ctx.accounts.vault_authority.key(),
            FusionError::InvalidPda
        );

        let vault_auth_seeds = &[
            b"vault_auth",
            market.to_account_info().key.as_ref(),
            &[market.vault_authority_bump],
        ];
        let signer = &[&vault_auth_seeds[..]];

        let mut taker_locked_quote = 0u64;
        let mut taker_locked_base = 0u64;

        match side {
            Side::Bid => {
                taker_locked_quote = limit_price
                    .checked_mul(quantity)
                    .ok_or(error!(FusionError::MathOverflow))?;
                transfer_checked(
                    CpiContext::new(
                        ctx.accounts.token_program.to_account_info(),
                        TransferChecked {
                            from: ctx.accounts.user_quote_ata.to_account_info(),
                            to: ctx.accounts.quote_vault.to_account_info(),
                            authority: ctx.accounts.user.to_account_info(),
                            mint: ctx.accounts.quote_mint.to_account_info(),
                        },
                    ),
                    taker_locked_quote,
                    ctx.accounts.quote_mint.decimals,
                )?;
            }
            Side::Ask => {
                taker_locked_base = quantity;
                transfer_checked(
                    CpiContext::new(
                        ctx.accounts.token_program.to_account_info(),
                        TransferChecked {
                            from: ctx.accounts.user_base_ata.to_account_info(),
                            to: ctx.accounts.base_vault.to_account_info(),
                            authority: ctx.accounts.user.to_account_info(),
                            mint: ctx.accounts.base_mint.to_account_info(),
                        },
                    ),
                    taker_locked_base,
                    ctx.accounts.base_mint.decimals,
                )?;
            }
        }

        while quantity > 0 {
            let best_idx = market.best_head(side.opposite());
            if best_idx == NONE_INDEX {
                break;
            }
            let best_usize = to_usize(best_idx)?;
            let maker = market.orders[best_usize];
            if !maker.is_live() {
                market.remove_from_book(best_idx)?;
                continue;
            }

            let crosses = match side {
                Side::Bid => maker.price <= limit_price,
                Side::Ask => maker.price >= limit_price,
            };
            if !crosses {
                break;
            }

            let trade_qty = quantity.min(maker.qty);
            let trade_quote = trade_qty
                .checked_mul(maker.price)
                .ok_or(error!(FusionError::MathOverflow))?;

            {
                let maker_mut = market.order_mut(best_idx)?;
                maker_mut.qty = maker_mut
                    .qty
                    .checked_sub(trade_qty)
                    .ok_or(error!(FusionError::MathOverflow))?;

                match side {
                    Side::Bid => {
                        maker_mut.locked_base = maker_mut
                            .locked_base
                            .checked_sub(trade_qty)
                            .ok_or(error!(FusionError::MathOverflow))?;
                        maker_mut.quote_claimable = maker_mut
                            .quote_claimable
                            .checked_add(trade_quote)
                            .ok_or(error!(FusionError::MathOverflow))?;
                        taker_locked_quote = taker_locked_quote
                            .checked_sub(trade_quote)
                            .ok_or(error!(FusionError::MathOverflow))?;
                    }
                    Side::Ask => {
                        maker_mut.locked_quote = maker_mut
                            .locked_quote
                            .checked_sub(trade_quote)
                            .ok_or(error!(FusionError::MathOverflow))?;
                        maker_mut.base_claimable = maker_mut
                            .base_claimable
                            .checked_add(trade_qty)
                            .ok_or(error!(FusionError::MathOverflow))?;
                        taker_locked_base = taker_locked_base
                            .checked_sub(trade_qty)
                            .ok_or(error!(FusionError::MathOverflow))?;
                    }
                }
            }

            match side {
                Side::Bid => {
                    transfer_checked(
                        CpiContext::new_with_signer(
                            ctx.accounts.token_program.to_account_info(),
                            TransferChecked {
                                from: ctx.accounts.base_vault.to_account_info(),
                                to: ctx.accounts.user_base_ata.to_account_info(),
                                authority: ctx.accounts.vault_authority.to_account_info(),
                                mint: ctx.accounts.base_mint.to_account_info(),
                            },
                            signer,
                        ),
                        trade_qty,
                        ctx.accounts.base_mint.decimals,
                    )?;
                }
                Side::Ask => {
                    transfer_checked(
                        CpiContext::new_with_signer(
                            ctx.accounts.token_program.to_account_info(),
                            TransferChecked {
                                from: ctx.accounts.quote_vault.to_account_info(),
                                to: ctx.accounts.user_quote_ata.to_account_info(),
                                authority: ctx.accounts.vault_authority.to_account_info(),
                                mint: ctx.accounts.quote_mint.to_account_info(),
                            },
                            signer,
                        ),
                        trade_quote,
                        ctx.accounts.quote_mint.decimals,
                    )?;
                }
            }

            quantity = quantity
                .checked_sub(trade_qty)
                .ok_or(error!(FusionError::MathOverflow))?;

            let now_filled = market.orders[best_usize].qty == 0;
            if now_filled {
                market.orders[best_usize].open = false;
                if market.orders[best_usize].in_book {
                    market.remove_from_book(best_idx)?;
                }
            }
        }

        if quantity > 0 {
            let new_idx = market.allocate_slot()?;
            let order_id = market.next_order_id;
            market.next_order_id = market
                .next_order_id
                .checked_add(1)
                .ok_or(error!(FusionError::MathOverflow))?;

            let mut new_order = OrderNode::default();
            new_order.used = true;
            new_order.open = true;
            new_order.in_book = true;
            new_order.owner = ctx.accounts.user.key();
            new_order.id = order_id;
            new_order.side = side as u8;
            new_order.price = limit_price;
            new_order.qty = quantity;
            new_order.next = NONE_INDEX;
            new_order.prev = NONE_INDEX;

            match side {
                Side::Bid => {
                    let required = limit_price
                        .checked_mul(quantity)
                        .ok_or(error!(FusionError::MathOverflow))?;
                    if taker_locked_quote > required {
                        let refund = taker_locked_quote
                            .checked_sub(required)
                            .ok_or(error!(FusionError::MathOverflow))?;
                        transfer_checked(
                            CpiContext::new_with_signer(
                                ctx.accounts.token_program.to_account_info(),
                                TransferChecked {
                                    from: ctx.accounts.quote_vault.to_account_info(),
                                    to: ctx.accounts.user_quote_ata.to_account_info(),
                                    authority: ctx.accounts.vault_authority.to_account_info(),
                                    mint: ctx.accounts.quote_mint.to_account_info(),
                                },
                                signer,
                            ),
                            refund,
                            ctx.accounts.quote_mint.decimals,
                        )?;
                        taker_locked_quote = required;
                    }
                    new_order.locked_quote = taker_locked_quote;
                }
                Side::Ask => {
                    new_order.locked_base = taker_locked_base;
                }
            }

            market.orders[to_usize(new_idx)?] = new_order;
            market.insert_into_book(new_idx)?;
            market.order_count = market
                .order_count
                .checked_add(1)
                .ok_or(error!(FusionError::MathOverflow))?;
        } else {
            match side {
                Side::Bid if taker_locked_quote > 0 => {
                    transfer_checked(
                        CpiContext::new_with_signer(
                            ctx.accounts.token_program.to_account_info(),
                            TransferChecked {
                                from: ctx.accounts.quote_vault.to_account_info(),
                                to: ctx.accounts.user_quote_ata.to_account_info(),
                                authority: ctx.accounts.vault_authority.to_account_info(),
                                mint: ctx.accounts.quote_mint.to_account_info(),
                            },
                            signer,
                        ),
                        taker_locked_quote,
                        ctx.accounts.quote_mint.decimals,
                    )?;
                }
                Side::Ask if taker_locked_base > 0 => {
                    transfer_checked(
                        CpiContext::new_with_signer(
                            ctx.accounts.token_program.to_account_info(),
                            TransferChecked {
                                from: ctx.accounts.base_vault.to_account_info(),
                                to: ctx.accounts.user_base_ata.to_account_info(),
                                authority: ctx.accounts.vault_authority.to_account_info(),
                                mint: ctx.accounts.base_mint.to_account_info(),
                            },
                            signer,
                        ),
                        taker_locked_base,
                        ctx.accounts.base_mint.decimals,
                    )?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    pub fn cancel_order(ctx: Context<CancelOrderCtx>, order_id: u64) -> Result<()> {
        let market = &mut ctx.accounts.market;
        cancel_order_internal(market, ctx.accounts.owner.key(), order_id)
    }

    pub fn claim_order_proceeds(
        ctx: Context<ClaimOrderProceedsCtx>,
        order_id: u64,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        let idx = market.find_order_index(order_id)?;
        let (base_out, quote_out, should_close_slot) =
            claim_order_proceeds_internal(market, idx, ctx.accounts.owner.key())?;

        let (expected_vault_authority, _) =
            Pubkey::find_program_address(&[b"vault_auth", market.key().as_ref()], ctx.program_id);
        require_keys_eq!(
            expected_vault_authority,
            ctx.accounts.vault_authority.key(),
            FusionError::InvalidPda
        );

        let market_key = market.key();
        let bump = market.vault_authority_bump;
        let vault_auth_seeds = &[b"vault_auth", market_key.as_ref(), &[bump]];
        let signer = &[&vault_auth_seeds[..]];

        if base_out > 0 {
            transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.base_vault.to_account_info(),
                        to: ctx.accounts.owner_base_ata.to_account_info(),
                        authority: ctx.accounts.vault_authority.to_account_info(),
                        mint: ctx.accounts.base_mint.to_account_info(),
                    },
                    signer,
                ),
                base_out,
                ctx.accounts.base_mint.decimals,
            )?;
        }

        if quote_out > 0 {
            transfer_checked(
                CpiContext::new_with_signer(
                    ctx.accounts.token_program.to_account_info(),
                    TransferChecked {
                        from: ctx.accounts.quote_vault.to_account_info(),
                        to: ctx.accounts.owner_quote_ata.to_account_info(),
                        authority: ctx.accounts.vault_authority.to_account_info(),
                        mint: ctx.accounts.quote_mint.to_account_info(),
                    },
                    signer,
                ),
                quote_out,
                ctx.accounts.quote_mint.decimals,
            )?;
        }

        if should_close_slot {
            market.free_slot(idx)?;
        }

        Ok(())
    }
}

fn cancel_order_internal(market: &mut Market, owner: Pubkey, order_id: u64) -> Result<()> {
    let idx = market.find_order_index(order_id)?;
    let snapshot = market.orders[to_usize(idx)?];
    require_keys_eq!(snapshot.owner, owner, FusionError::Unauthorized);
    require!(snapshot.open, FusionError::OrderNotOpen);

    if snapshot.in_book {
        market.remove_from_book(idx)?;
    }
    market.order_mut(idx)?.open = false;
    Ok(())
}

fn claim_order_proceeds_internal(
    market: &mut Market,
    idx: i16,
    owner: Pubkey,
) -> Result<(u64, u64, bool)> {
    let order = market.order_mut(idx)?;
    require_keys_eq!(order.owner, owner, FusionError::Unauthorized);

    let base_out = order
        .base_claimable
        .checked_add(if order.open { 0 } else { order.locked_base })
        .ok_or(error!(FusionError::MathOverflow))?;
    let quote_out = order
        .quote_claimable
        .checked_add(if order.open { 0 } else { order.locked_quote })
        .ok_or(error!(FusionError::MathOverflow))?;

    order.base_claimable = 0;
    order.quote_claimable = 0;
    if !order.open {
        order.locked_base = 0;
        order.locked_quote = 0;
    }

    Ok((
        base_out,
        quote_out,
        !order.open && order.base_claimable == 0 && order.quote_claimable == 0,
    ))
}

#[derive(Accounts)]
#[instruction(market_bump: u8, _vault_authority_bump: u8)]
pub struct InitializeMarketCtx<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,
    #[account(
        init,
        payer = admin,
        space = MARKET_SPACE,
        seeds = [b"market", admin.key().as_ref()],
        bump
    )]
    pub market: Account<'info, Market>,
    pub base_mint: InterfaceAccount<'info, Mint>,
    pub quote_mint: InterfaceAccount<'info, Mint>,
    #[account(
        init,
        payer = admin,
        token::mint = base_mint,
        token::authority = vault_authority,
    )]
    pub base_vault: InterfaceAccount<'info, TokenAccount>,
    #[account(
        init,
        payer = admin,
        token::mint = quote_mint,
        token::authority = vault_authority,
    )]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,
    /// CHECK: PDA checked in instruction body.
    #[account(
        seeds = [b"vault_auth", market.key().as_ref()],
        bump = _vault_authority_bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct PlaceOrderCtx<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(
        mut,
        has_one = base_mint,
        has_one = quote_mint,
        has_one = base_vault,
        has_one = quote_vault
    )]
    pub market: Account<'info, Market>,
    pub base_mint: InterfaceAccount<'info, Mint>,
    pub quote_mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub base_vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        constraint = user_base_ata.owner == user.key(),
        constraint = user_base_ata.mint == base_mint.key()
    )]
    pub user_base_ata: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        constraint = user_quote_ata.owner == user.key(),
        constraint = user_quote_ata.mint == quote_mint.key()
    )]
    pub user_quote_ata: InterfaceAccount<'info, TokenAccount>,
    /// CHECK: PDA checked in initialize and implied by market.
    pub vault_authority: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct CancelOrderCtx<'info> {
    pub owner: Signer<'info>,
    #[account(mut)]
    pub market: Account<'info, Market>,
}

#[derive(Accounts)]
pub struct ClaimOrderProceedsCtx<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    #[account(
        mut,
        has_one = base_mint,
        has_one = quote_mint,
        has_one = base_vault,
        has_one = quote_vault
    )]
    pub market: Account<'info, Market>,
    pub base_mint: InterfaceAccount<'info, Mint>,
    pub quote_mint: InterfaceAccount<'info, Mint>,
    #[account(mut)]
    pub base_vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        constraint = owner_base_ata.owner == owner.key(),
        constraint = owner_base_ata.mint == base_mint.key()
    )]
    pub owner_base_ata: InterfaceAccount<'info, TokenAccount>,
    #[account(
        mut,
        constraint = owner_quote_ata.owner == owner.key(),
        constraint = owner_quote_ata.mint == quote_mint.key()
    )]
    pub owner_quote_ata: InterfaceAccount<'info, TokenAccount>,
    /// CHECK: PDA derived from market.
    pub vault_authority: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[account]
pub struct Market {
    pub admin: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub bump: u8,
    pub vault_authority_bump: u8,
    pub bids_head: i16,
    pub asks_head: i16,
    pub free_head: i16,
    pub next_order_id: u64,
    pub order_count: u16,
    pub orders: [OrderNode; MAX_ORDERS],
}

impl Market {
    fn best_head(&self, side: Side) -> i16 {
        match side {
            Side::Bid => self.bids_head,
            Side::Ask => self.asks_head,
        }
    }

    fn head_mut(&mut self, side: Side) -> &mut i16 {
        match side {
            Side::Bid => &mut self.bids_head,
            Side::Ask => &mut self.asks_head,
        }
    }

    fn allocate_slot(&mut self) -> Result<i16> {
        require!(self.free_head != NONE_INDEX, FusionError::BookFull);
        let idx = self.free_head;
        let next = self.orders[to_usize(idx)?].next;
        self.free_head = next;
        Ok(idx)
    }

    fn free_slot(&mut self, idx: i16) -> Result<()> {
        let next_free = self.free_head;
        let slot = self.order_mut(idx)?;
        *slot = OrderNode {
            next: next_free,
            ..OrderNode::default()
        };
        self.free_head = idx;
        self.order_count = self
            .order_count
            .checked_sub(1)
            .ok_or(error!(FusionError::MathOverflow))?;
        Ok(())
    }

    fn order_mut(&mut self, idx: i16) -> Result<&mut OrderNode> {
        Ok(&mut self.orders[to_usize(idx)?])
    }

    fn find_order_index(&self, order_id: u64) -> Result<i16> {
        for i in 0..MAX_ORDERS {
            let slot = self.orders[i];
            if slot.used && slot.id == order_id {
                return Ok(i as i16);
            }
        }
        err!(FusionError::OrderNotFound)
    }

    fn remove_from_book(&mut self, idx: i16) -> Result<()> {
        let (side, prev, next, in_book) = {
            let slot = self.order_mut(idx)?;
            (
                slot.side_enum().map_err(map_engine_error)?,
                slot.prev,
                slot.next,
                slot.in_book,
            )
        };
        require!(in_book, FusionError::OrderNotOpen);

        if prev == NONE_INDEX {
            *self.head_mut(side) = next;
        } else {
            self.order_mut(prev)?.next = next;
        }
        if next != NONE_INDEX {
            self.order_mut(next)?.prev = prev;
        }

        let slot = self.order_mut(idx)?;
        slot.prev = NONE_INDEX;
        slot.next = NONE_INDEX;
        slot.in_book = false;
        Ok(())
    }

    fn insert_into_book(&mut self, idx: i16) -> Result<()> {
        let (side, price, id) = {
            let slot = self.order_mut(idx)?;
            (
                slot.side_enum().map_err(map_engine_error)?,
                slot.price,
                slot.id,
            )
        };

        let mut current = self.best_head(side);
        let mut prev = NONE_INDEX;

        while current != NONE_INDEX {
            let c = self.orders[to_usize(current)?];
            let better = match side {
                Side::Bid => price > c.price || (price == c.price && id < c.id),
                Side::Ask => price < c.price || (price == c.price && id < c.id),
            };
            if better {
                break;
            }
            prev = current;
            current = c.next;
        }

        {
            let slot = self.order_mut(idx)?;
            slot.prev = prev;
            slot.next = current;
            slot.in_book = true;
        }

        if prev == NONE_INDEX {
            *self.head_mut(side) = idx;
        } else {
            self.order_mut(prev)?.next = idx;
        }
        if current != NONE_INDEX {
            self.order_mut(current)?.prev = idx;
        }

        Ok(())
    }
}

fn to_usize(idx: i16) -> Result<usize> {
    require!(idx >= 0, FusionError::IndexOutOfBounds);
    let u = idx as usize;
    require!(u < MAX_ORDERS, FusionError::IndexOutOfBounds);
    Ok(u)
}

#[error_code]
pub enum FusionError {
    #[msg("Invalid side value")]
    InvalidSide,
    #[msg("Invalid limit price")]
    InvalidPrice,
    #[msg("Invalid quantity")]
    InvalidQuantity,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Book is full")]
    BookFull,
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("Order not found")]
    OrderNotFound,
    #[msg("Order is not open")]
    OrderNotOpen,
    #[msg("Invalid PDA")]
    InvalidPda,
    #[msg("Index out of bounds")]
    IndexOutOfBounds,
}

fn map_engine_error(err: fusion_engine::FusionEngineError) -> Error {
    match err {
        fusion_engine::FusionEngineError::InvalidSide => error!(FusionError::InvalidSide),
        fusion_engine::FusionEngineError::InvalidInstructionTag
        | fusion_engine::FusionEngineError::InvalidInstructionData => {
            error!(FusionError::InvalidSide)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_test_market() -> Market {
        let mut market = Market {
            admin: Pubkey::new_unique(),
            base_mint: Pubkey::new_unique(),
            quote_mint: Pubkey::new_unique(),
            base_vault: Pubkey::new_unique(),
            quote_vault: Pubkey::new_unique(),
            bump: 1,
            vault_authority_bump: 255,
            bids_head: NONE_INDEX,
            asks_head: NONE_INDEX,
            free_head: 0,
            next_order_id: 1,
            order_count: 0,
            orders: [OrderNode::default(); MAX_ORDERS],
        };

        for i in 0..MAX_ORDERS {
            market.orders[i].next = if i + 1 < MAX_ORDERS {
                (i as i16) + 1
            } else {
                NONE_INDEX
            };
        }
        market
    }

    fn simulate_place_order_state(
        market: &mut Market,
        user: Pubkey,
        side: Side,
        limit_price: u64,
        mut quantity: u64,
    ) -> Result<Option<u64>> {
        let mut taker_locked_quote = 0u64;
        let mut taker_locked_base = 0u64;

        match side {
            Side::Bid => {
                taker_locked_quote = limit_price
                    .checked_mul(quantity)
                    .ok_or(error!(FusionError::MathOverflow))?;
            }
            Side::Ask => taker_locked_base = quantity,
        }

        while quantity > 0 {
            let best_idx = market.best_head(side.opposite());
            if best_idx == NONE_INDEX {
                break;
            }
            let best_usize = to_usize(best_idx)?;
            let maker = market.orders[best_usize];
            if !maker.is_live() {
                market.remove_from_book(best_idx)?;
                continue;
            }

            let crosses = match side {
                Side::Bid => maker.price <= limit_price,
                Side::Ask => maker.price >= limit_price,
            };
            if !crosses {
                break;
            }

            let trade_qty = quantity.min(maker.qty);
            let trade_quote = trade_qty
                .checked_mul(maker.price)
                .ok_or(error!(FusionError::MathOverflow))?;

            {
                let maker_mut = market.order_mut(best_idx)?;
                maker_mut.qty = maker_mut
                    .qty
                    .checked_sub(trade_qty)
                    .ok_or(error!(FusionError::MathOverflow))?;

                match side {
                    Side::Bid => {
                        maker_mut.locked_base = maker_mut
                            .locked_base
                            .checked_sub(trade_qty)
                            .ok_or(error!(FusionError::MathOverflow))?;
                        maker_mut.quote_claimable = maker_mut
                            .quote_claimable
                            .checked_add(trade_quote)
                            .ok_or(error!(FusionError::MathOverflow))?;
                        taker_locked_quote = taker_locked_quote
                            .checked_sub(trade_quote)
                            .ok_or(error!(FusionError::MathOverflow))?;
                    }
                    Side::Ask => {
                        maker_mut.locked_quote = maker_mut
                            .locked_quote
                            .checked_sub(trade_quote)
                            .ok_or(error!(FusionError::MathOverflow))?;
                        maker_mut.base_claimable = maker_mut
                            .base_claimable
                            .checked_add(trade_qty)
                            .ok_or(error!(FusionError::MathOverflow))?;
                        taker_locked_base = taker_locked_base
                            .checked_sub(trade_qty)
                            .ok_or(error!(FusionError::MathOverflow))?;
                    }
                }
            }

            quantity = quantity
                .checked_sub(trade_qty)
                .ok_or(error!(FusionError::MathOverflow))?;

            let now_filled = market.orders[best_usize].qty == 0;
            if now_filled {
                market.orders[best_usize].open = false;
                if market.orders[best_usize].in_book {
                    market.remove_from_book(best_idx)?;
                }
            }
        }

        if quantity == 0 {
            return Ok(None);
        }

        let new_idx = market.allocate_slot()?;
        let order_id = market.next_order_id;
        market.next_order_id = market
            .next_order_id
            .checked_add(1)
            .ok_or(error!(FusionError::MathOverflow))?;

        let mut new_order = OrderNode::default();
        new_order.used = true;
        new_order.open = true;
        new_order.in_book = true;
        new_order.owner = user;
        new_order.id = order_id;
        new_order.side = side as u8;
        new_order.price = limit_price;
        new_order.qty = quantity;
        new_order.next = NONE_INDEX;
        new_order.prev = NONE_INDEX;

        match side {
            Side::Bid => {
                let required = limit_price
                    .checked_mul(quantity)
                    .ok_or(error!(FusionError::MathOverflow))?;
                if taker_locked_quote > required {
                    taker_locked_quote = required;
                }
                new_order.locked_quote = taker_locked_quote;
            }
            Side::Ask => new_order.locked_base = taker_locked_base,
        }

        market.orders[to_usize(new_idx)?] = new_order;
        market.insert_into_book(new_idx)?;
        market.order_count = market
            .order_count
            .checked_add(1)
            .ok_or(error!(FusionError::MathOverflow))?;

        Ok(Some(order_id))
    }

    #[test]
    fn partial_fill_then_resting_maker_state_is_preserved() {
        let mut market = new_test_market();
        let maker = Pubkey::new_unique();
        let taker = Pubkey::new_unique();

        let maker_order_id =
            simulate_place_order_state(&mut market, maker, Side::Ask, 100, 10).unwrap().unwrap();
        let taker_order_id =
            simulate_place_order_state(&mut market, taker, Side::Bid, 100, 4).unwrap();

        assert!(taker_order_id.is_none());

        let idx = market.find_order_index(maker_order_id).unwrap();
        let order = market.orders[to_usize(idx).unwrap()];
        assert_eq!(order.qty, 6);
        assert_eq!(order.locked_base, 6);
        assert_eq!(order.quote_claimable, 400);
        assert!(order.open);
        assert!(order.in_book);
    }

    #[test]
    fn full_fill_closes_maker_order() {
        let mut market = new_test_market();
        let maker = Pubkey::new_unique();
        let taker = Pubkey::new_unique();

        let maker_order_id =
            simulate_place_order_state(&mut market, maker, Side::Ask, 100, 5).unwrap().unwrap();
        let _ = simulate_place_order_state(&mut market, taker, Side::Bid, 100, 5).unwrap();

        let idx = market.find_order_index(maker_order_id).unwrap();
        let order = market.orders[to_usize(idx).unwrap()];
        assert_eq!(order.qty, 0);
        assert_eq!(order.locked_base, 0);
        assert_eq!(order.quote_claimable, 500);
        assert!(!order.open);
        assert!(!order.in_book);
    }

    #[test]
    fn cancel_after_partial_fill_and_claim_is_single_use() {
        let mut market = new_test_market();
        let maker = Pubkey::new_unique();
        let taker = Pubkey::new_unique();

        let maker_order_id =
            simulate_place_order_state(&mut market, maker, Side::Ask, 100, 10).unwrap().unwrap();
        let _ = simulate_place_order_state(&mut market, taker, Side::Bid, 100, 4).unwrap();

        cancel_order_internal(&mut market, maker, maker_order_id).unwrap();
        let idx = market.find_order_index(maker_order_id).unwrap();
        let (base_out, quote_out, should_close) =
            claim_order_proceeds_internal(&mut market, idx, maker).unwrap();
        assert_eq!(base_out, 6);
        assert_eq!(quote_out, 400);
        assert!(should_close);

        market.free_slot(idx).unwrap();
        let second = market.find_order_index(maker_order_id);
        assert!(second.is_err());
    }
}
