#![cfg_attr(not(feature = "bpf-entrypoint"), allow(dead_code))]

use borsh::{BorshDeserialize, BorshSerialize};
use fusion_engine::{
    CancelOrderArgs, ClaimOrderProceedsArgs, InitializeMarketArgs, OrderNode, PlaceOrderArgs, Side,
    MAX_ORDERS, NONE_INDEX,
};
use pinocchio::{
    address::{self, Address},
    AccountView, ProgramResult,
};
use pinocchio_token_2022::instructions::TransferChecked;
use solana_instruction_view::cpi::{Seed, Signer};
use solana_program_error::ProgramError;

address::declare_id!("Fus1on1111111111111111111111111111111111111");

const MARKET_DISCRIMINATOR: [u8; 8] = [0xdb, 0xbe, 0xd5, 0x37, 0x00, 0xe3, 0xc6, 0x9a];
const IX_INITIALIZE_MARKET: [u8; 8] = [0x23, 0x23, 0xbd, 0xc1, 0x9b, 0x30, 0xaa, 0xcb];
const IX_PLACE_ORDER: [u8; 8] = [0x33, 0xc2, 0x9b, 0xaf, 0x6d, 0x82, 0x60, 0x6a];
const IX_CANCEL_ORDER: [u8; 8] = [0x5f, 0x81, 0xed, 0xf0, 0x08, 0x31, 0xdf, 0x84];
const IX_CLAIM_ORDER_PROCEEDS: [u8; 8] = [0xfe, 0xb0, 0xc8, 0x20, 0x78, 0x1c, 0x5c, 0x34];

const TOKEN_PROGRAM_LEGACY: Address =
    Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const TOKEN_PROGRAM_2022: Address =
    Address::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

const MINT_BASE_LEN: usize = 82;
const TOKEN_ACCOUNT_BASE_LEN: usize = 165;

#[cfg(feature = "bpf-entrypoint")]
pinocchio::entrypoint!(process_instruction);

type Result<T> = core::result::Result<T, ProgramError>;

#[derive(Clone, Copy)]
enum FusionError {
    InvalidSide = 1,
    InvalidPrice = 2,
    InvalidQuantity = 3,
    MathOverflow = 4,
    BookFull = 5,
    Unauthorized = 6,
    OrderNotFound = 7,
    OrderNotOpen = 8,
    InvalidPda = 9,
    IndexOutOfBounds = 10,
    InvalidInstructionData = 11,
    InvalidAccountOrder = 12,
    DuplicateMutableAccount = 13,
    TokenExtensionUnsupported = 14,
}

fn fusion_err(code: FusionError) -> ProgramError {
    ProgramError::Custom(code as u32)
}

macro_rules! require {
    ($cond:expr, $err:expr) => {
        if !$cond {
            return Err(fusion_err($err));
        }
    };
}

macro_rules! require_eq {
    ($a:expr, $b:expr, $err:expr) => {
        if $a != $b {
            return Err(fusion_err($err));
        }
    };
}

macro_rules! require_gt {
    ($a:expr, $b:expr, $err:expr) => {
        if $a <= $b {
            return Err(fusion_err($err));
        }
    };
}

#[inline(never)]
fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    instruction_data: &[u8],
) -> ProgramResult {
    require_eq!(program_id, &id(), FusionError::InvalidInstructionData);
    require!(instruction_data.len() >= 8, FusionError::InvalidInstructionData);

    let mut discr = [0u8; 8];
    discr.copy_from_slice(&instruction_data[..8]);
    let payload = &instruction_data[8..];

    if discr == IX_INITIALIZE_MARKET {
        let args = InitializeMarketArgs::try_from_slice(payload)
            .map_err(|_| fusion_err(FusionError::InvalidInstructionData))?;
        return initialize_market(accounts, args);
    }

    if discr == IX_PLACE_ORDER {
        let args = PlaceOrderArgs::try_from_slice(payload)
            .map_err(|_| fusion_err(FusionError::InvalidInstructionData))?;
        return place_order(accounts, args);
    }

    if discr == IX_CANCEL_ORDER {
        let args = CancelOrderArgs::try_from_slice(payload)
            .map_err(|_| fusion_err(FusionError::InvalidInstructionData))?;
        return cancel_order(accounts, args);
    }

    if discr == IX_CLAIM_ORDER_PROCEEDS {
        let args = ClaimOrderProceedsArgs::try_from_slice(payload)
            .map_err(|_| fusion_err(FusionError::InvalidInstructionData))?;
        return claim_order_proceeds(accounts, args);
    }

    Err(ProgramError::InvalidInstructionData)
}

fn initialize_market(accounts: &mut [AccountView], args: InitializeMarketArgs) -> ProgramResult {
    require!(accounts.len() >= 9, FusionError::InvalidAccountOrder);

    let admin = &accounts[0];
    let market_ai = &accounts[1];
    let base_mint = &accounts[2];
    let quote_mint = &accounts[3];
    let base_vault = &accounts[4];
    let quote_vault = &accounts[5];
    let vault_authority = &accounts[6];
    let token_program = &accounts[7];
    let _system_program = &accounts[8];

    require!(admin.is_signer(), FusionError::Unauthorized);
    require!(admin.is_writable(), FusionError::InvalidAccountOrder);
    require!(market_ai.is_writable(), FusionError::InvalidAccountOrder);
    require!(base_vault.is_writable(), FusionError::InvalidAccountOrder);
    require!(quote_vault.is_writable(), FusionError::InvalidAccountOrder);
    ensure_unique_mutable(&[admin, market_ai, base_vault, quote_vault])?;

    let token_program_id = validate_token_program(token_program)?;

    let expected_market =
        Address::derive_address(&[b"market", admin.address().as_ref()], Some(args.market_bump), &id());
    require_eq!(
        &expected_market,
        market_ai.address(),
        FusionError::InvalidPda
    );

    let (expected_vault_auth, derived_bump) =
        Address::derive_program_address(&[b"vault_auth", market_ai.address().as_ref()], &id())
            .ok_or(fusion_err(FusionError::InvalidPda))?;
    require_eq!(
        &expected_vault_auth,
        vault_authority.address(),
        FusionError::InvalidPda
    );
    require_eq!(
        args.vault_authority_bump,
        derived_bump,
        FusionError::InvalidPda
    );

    validate_mint(base_mint, &token_program_id)?;
    validate_mint(quote_mint, &token_program_id)?;
    validate_token_account(base_vault, &token_program_id, base_mint.address(), vault_authority.address())?;
    validate_token_account(
        quote_vault,
        &token_program_id,
        quote_mint.address(),
        vault_authority.address(),
    )?;

    let mut market = Market::default();
    market.admin = addr_to_pubkey(admin.address());
    market.base_mint = addr_to_pubkey(base_mint.address());
    market.quote_mint = addr_to_pubkey(quote_mint.address());
    market.base_vault = addr_to_pubkey(base_vault.address());
    market.quote_vault = addr_to_pubkey(quote_vault.address());
    market.bump = args.market_bump;
    market.vault_authority_bump = args.vault_authority_bump;
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

    store_market(accounts, 1, &market)?;
    Ok(())
}

fn place_order(accounts: &mut [AccountView], args: PlaceOrderArgs) -> ProgramResult {
    require!(accounts.len() >= 10, FusionError::InvalidAccountOrder);

    let user = &accounts[0];
    let market_ai = &accounts[1];
    let base_mint = &accounts[2];
    let quote_mint = &accounts[3];
    let base_vault = &accounts[4];
    let quote_vault = &accounts[5];
    let user_base_ata = &accounts[6];
    let user_quote_ata = &accounts[7];
    let vault_authority = &accounts[8];
    let token_program = &accounts[9];

    require!(user.is_signer(), FusionError::Unauthorized);
    require!(user.is_writable(), FusionError::InvalidAccountOrder);
    require!(market_ai.is_writable(), FusionError::InvalidAccountOrder);
    require!(base_vault.is_writable(), FusionError::InvalidAccountOrder);
    require!(quote_vault.is_writable(), FusionError::InvalidAccountOrder);
    require!(user_base_ata.is_writable(), FusionError::InvalidAccountOrder);
    require!(user_quote_ata.is_writable(), FusionError::InvalidAccountOrder);

    ensure_unique_mutable(&[
        user,
        market_ai,
        base_vault,
        quote_vault,
        user_base_ata,
        user_quote_ata,
    ])?;

    let token_program_id = validate_token_program(token_program)?;
    let base_decimals = validate_mint(base_mint, &token_program_id)?;
    let quote_decimals = validate_mint(quote_mint, &token_program_id)?;

    let mut market = load_market(market_ai)?;
    require_eq!(market.base_mint, addr_to_pubkey(base_mint.address()), FusionError::InvalidAccountOrder);
    require_eq!(market.quote_mint, addr_to_pubkey(quote_mint.address()), FusionError::InvalidAccountOrder);
    require_eq!(market.base_vault, addr_to_pubkey(base_vault.address()), FusionError::InvalidAccountOrder);
    require_eq!(market.quote_vault, addr_to_pubkey(quote_vault.address()), FusionError::InvalidAccountOrder);

    let (expected_vault_authority, _) =
        Address::derive_program_address(&[b"vault_auth", market_ai.address().as_ref()], &id())
            .ok_or(fusion_err(FusionError::InvalidPda))?;
    require_eq!(
        &expected_vault_authority,
        vault_authority.address(),
        FusionError::InvalidPda
    );

    validate_token_account(base_vault, &token_program_id, base_mint.address(), vault_authority.address())?;
    validate_token_account(
        quote_vault,
        &token_program_id,
        quote_mint.address(),
        vault_authority.address(),
    )?;
    validate_token_account(user_base_ata, &token_program_id, base_mint.address(), user.address())?;
    validate_token_account(user_quote_ata, &token_program_id, quote_mint.address(), user.address())?;

    require!(
        args.side == Side::Bid as u8 || args.side == Side::Ask as u8,
        FusionError::InvalidSide
    );
    require_gt!(args.limit_price, 0, FusionError::InvalidPrice);
    require_gt!(args.quantity, 0, FusionError::InvalidQuantity);

    let side = Side::try_from_u8(args.side).map_err(map_engine_error)?;
    let mut quantity = args.quantity;

    let mut taker_locked_quote = 0u64;
    let mut taker_locked_base = 0u64;

    let bump_seed = [market.vault_authority_bump];
    let vault_signer_seeds = [
        Seed::from(b"vault_auth".as_slice()),
        Seed::from(market_ai.address().as_ref()),
        Seed::from(&bump_seed),
    ];
    let vault_signers = [Signer::from(&vault_signer_seeds)];

    match side {
        Side::Bid => {
            taker_locked_quote = args
                .limit_price
                .checked_mul(quantity)
                .ok_or(fusion_err(FusionError::MathOverflow))?;
            TransferChecked {
                from: user_quote_ata,
                mint: quote_mint,
                to: quote_vault,
                authority: user,
                amount: taker_locked_quote,
                decimals: quote_decimals,
                token_program: &token_program_id,
            }
            .invoke()?;
        }
        Side::Ask => {
            taker_locked_base = quantity;
            TransferChecked {
                from: user_base_ata,
                mint: base_mint,
                to: base_vault,
                authority: user,
                amount: taker_locked_base,
                decimals: base_decimals,
                token_program: &token_program_id,
            }
            .invoke()?;
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
            Side::Bid => maker.price <= args.limit_price,
            Side::Ask => maker.price >= args.limit_price,
        };
        if !crosses {
            break;
        }

        let trade_qty = quantity.min(maker.qty);
        let trade_quote = trade_qty
            .checked_mul(maker.price)
            .ok_or(fusion_err(FusionError::MathOverflow))?;

        {
            let maker_mut = market.order_mut(best_idx)?;
            maker_mut.qty = maker_mut
                .qty
                .checked_sub(trade_qty)
                .ok_or(fusion_err(FusionError::MathOverflow))?;

            match side {
                Side::Bid => {
                    maker_mut.locked_base = maker_mut
                        .locked_base
                        .checked_sub(trade_qty)
                        .ok_or(fusion_err(FusionError::MathOverflow))?;
                    maker_mut.quote_claimable = maker_mut
                        .quote_claimable
                        .checked_add(trade_quote)
                        .ok_or(fusion_err(FusionError::MathOverflow))?;
                    taker_locked_quote = taker_locked_quote
                        .checked_sub(trade_quote)
                        .ok_or(fusion_err(FusionError::MathOverflow))?;
                }
                Side::Ask => {
                    maker_mut.locked_quote = maker_mut
                        .locked_quote
                        .checked_sub(trade_quote)
                        .ok_or(fusion_err(FusionError::MathOverflow))?;
                    maker_mut.base_claimable = maker_mut
                        .base_claimable
                        .checked_add(trade_qty)
                        .ok_or(fusion_err(FusionError::MathOverflow))?;
                    taker_locked_base = taker_locked_base
                        .checked_sub(trade_qty)
                        .ok_or(fusion_err(FusionError::MathOverflow))?;
                }
            }
        }

        match side {
            Side::Bid => {
                TransferChecked {
                    from: base_vault,
                    mint: base_mint,
                    to: user_base_ata,
                    authority: vault_authority,
                    amount: trade_qty,
                    decimals: base_decimals,
                    token_program: &token_program_id,
                }
                .invoke_signed(&vault_signers)?;
            }
            Side::Ask => {
                TransferChecked {
                    from: quote_vault,
                    mint: quote_mint,
                    to: user_quote_ata,
                    authority: vault_authority,
                    amount: trade_quote,
                    decimals: quote_decimals,
                    token_program: &token_program_id,
                }
                .invoke_signed(&vault_signers)?;
            }
        }

        quantity = quantity
            .checked_sub(trade_qty)
            .ok_or(fusion_err(FusionError::MathOverflow))?;

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
            .ok_or(fusion_err(FusionError::MathOverflow))?;

        let mut new_order = OrderNode::default();
        new_order.used = true;
        new_order.open = true;
        new_order.in_book = true;
        new_order.owner = addr_to_pubkey(user.address());
        new_order.id = order_id;
        new_order.side = side as u8;
        new_order.price = args.limit_price;
        new_order.qty = quantity;
        new_order.next = NONE_INDEX;
        new_order.prev = NONE_INDEX;

        match side {
            Side::Bid => {
                let required = args
                    .limit_price
                    .checked_mul(quantity)
                    .ok_or(fusion_err(FusionError::MathOverflow))?;
                if taker_locked_quote > required {
                    let refund = taker_locked_quote
                        .checked_sub(required)
                        .ok_or(fusion_err(FusionError::MathOverflow))?;
                    TransferChecked {
                        from: quote_vault,
                        mint: quote_mint,
                        to: user_quote_ata,
                        authority: vault_authority,
                        amount: refund,
                        decimals: quote_decimals,
                        token_program: &token_program_id,
                    }
                    .invoke_signed(&vault_signers)?;
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
            .ok_or(fusion_err(FusionError::MathOverflow))?;
    } else {
        match side {
            Side::Bid if taker_locked_quote > 0 => {
                TransferChecked {
                    from: quote_vault,
                    mint: quote_mint,
                    to: user_quote_ata,
                    authority: vault_authority,
                    amount: taker_locked_quote,
                    decimals: quote_decimals,
                    token_program: &token_program_id,
                }
                .invoke_signed(&vault_signers)?;
            }
            Side::Ask if taker_locked_base > 0 => {
                TransferChecked {
                    from: base_vault,
                    mint: base_mint,
                    to: user_base_ata,
                    authority: vault_authority,
                    amount: taker_locked_base,
                    decimals: base_decimals,
                    token_program: &token_program_id,
                }
                .invoke_signed(&vault_signers)?;
            }
            _ => {}
        }
    }

    store_market(accounts, 1, &market)?;
    Ok(())
}

fn cancel_order(accounts: &mut [AccountView], args: CancelOrderArgs) -> ProgramResult {
    require!(accounts.len() >= 2, FusionError::InvalidAccountOrder);

    let owner = &accounts[0];
    let market_ai = &accounts[1];

    require!(owner.is_signer(), FusionError::Unauthorized);
    require!(market_ai.is_writable(), FusionError::InvalidAccountOrder);

    let mut market = load_market(market_ai)?;
    cancel_order_internal(&mut market, addr_to_pubkey(owner.address()), args.order_id)?;
    store_market(accounts, 1, &market)?;
    Ok(())
}

fn claim_order_proceeds(
    accounts: &mut [AccountView],
    args: ClaimOrderProceedsArgs,
) -> ProgramResult {
    require!(accounts.len() >= 10, FusionError::InvalidAccountOrder);

    let owner = &accounts[0];
    let market_ai = &accounts[1];
    let base_mint = &accounts[2];
    let quote_mint = &accounts[3];
    let base_vault = &accounts[4];
    let quote_vault = &accounts[5];
    let owner_base_ata = &accounts[6];
    let owner_quote_ata = &accounts[7];
    let vault_authority = &accounts[8];
    let token_program = &accounts[9];

    require!(owner.is_signer(), FusionError::Unauthorized);
    require!(owner.is_writable(), FusionError::InvalidAccountOrder);
    require!(market_ai.is_writable(), FusionError::InvalidAccountOrder);
    require!(base_vault.is_writable(), FusionError::InvalidAccountOrder);
    require!(quote_vault.is_writable(), FusionError::InvalidAccountOrder);
    require!(owner_base_ata.is_writable(), FusionError::InvalidAccountOrder);
    require!(owner_quote_ata.is_writable(), FusionError::InvalidAccountOrder);

    ensure_unique_mutable(&[
        owner,
        market_ai,
        base_vault,
        quote_vault,
        owner_base_ata,
        owner_quote_ata,
    ])?;

    let token_program_id = validate_token_program(token_program)?;
    let base_decimals = validate_mint(base_mint, &token_program_id)?;
    let quote_decimals = validate_mint(quote_mint, &token_program_id)?;

    let mut market = load_market(market_ai)?;
    require_eq!(market.base_mint, addr_to_pubkey(base_mint.address()), FusionError::InvalidAccountOrder);
    require_eq!(market.quote_mint, addr_to_pubkey(quote_mint.address()), FusionError::InvalidAccountOrder);
    require_eq!(market.base_vault, addr_to_pubkey(base_vault.address()), FusionError::InvalidAccountOrder);
    require_eq!(market.quote_vault, addr_to_pubkey(quote_vault.address()), FusionError::InvalidAccountOrder);

    let (expected_vault_authority, _) =
        Address::derive_program_address(&[b"vault_auth", market_ai.address().as_ref()], &id())
            .ok_or(fusion_err(FusionError::InvalidPda))?;
    require_eq!(
        &expected_vault_authority,
        vault_authority.address(),
        FusionError::InvalidPda
    );

    validate_token_account(base_vault, &token_program_id, base_mint.address(), vault_authority.address())?;
    validate_token_account(
        quote_vault,
        &token_program_id,
        quote_mint.address(),
        vault_authority.address(),
    )?;
    validate_token_account(owner_base_ata, &token_program_id, base_mint.address(), owner.address())?;
    validate_token_account(
        owner_quote_ata,
        &token_program_id,
        quote_mint.address(),
        owner.address(),
    )?;

    let idx = market.find_order_index(args.order_id)?;
    let (base_out, quote_out, should_close_slot) =
        claim_order_proceeds_internal(&mut market, idx, addr_to_pubkey(owner.address()))?;

    let bump_seed = [market.vault_authority_bump];
    let vault_signer_seeds = [
        Seed::from(b"vault_auth".as_slice()),
        Seed::from(market_ai.address().as_ref()),
        Seed::from(&bump_seed),
    ];
    let vault_signers = [Signer::from(&vault_signer_seeds)];

    if base_out > 0 {
        TransferChecked {
            from: base_vault,
            mint: base_mint,
            to: owner_base_ata,
            authority: vault_authority,
            amount: base_out,
            decimals: base_decimals,
            token_program: &token_program_id,
        }
        .invoke_signed(&vault_signers)?;
    }

    if quote_out > 0 {
        TransferChecked {
            from: quote_vault,
            mint: quote_mint,
            to: owner_quote_ata,
            authority: vault_authority,
            amount: quote_out,
            decimals: quote_decimals,
            token_program: &token_program_id,
        }
        .invoke_signed(&vault_signers)?;
    }

    if should_close_slot {
        market.free_slot(idx)?;
    }

    store_market(accounts, 1, &market)?;
    Ok(())
}

fn ensure_unique_mutable(accounts: &[&AccountView]) -> ProgramResult {
    for i in 0..accounts.len() {
        for j in (i + 1)..accounts.len() {
            if accounts[i].address() == accounts[j].address() {
                return Err(fusion_err(FusionError::DuplicateMutableAccount));
            }
        }
    }
    Ok(())
}

fn validate_token_program(token_program: &AccountView) -> Result<Address> {
    let key = *token_program.address();
    if key == TOKEN_PROGRAM_LEGACY || key == TOKEN_PROGRAM_2022 {
        Ok(key)
    } else {
        Err(ProgramError::IncorrectProgramId)
    }
}

fn validate_mint(mint: &AccountView, token_program: &Address) -> Result<u8> {
    require_eq!(mint.owner(), token_program, FusionError::InvalidAccountOrder);
    let data = mint.try_borrow()?;
    require!(data.len() >= MINT_BASE_LEN, FusionError::InvalidAccountOrder);

    if *token_program == TOKEN_PROGRAM_2022 && data.len() > MINT_BASE_LEN {
        return Err(fusion_err(FusionError::TokenExtensionUnsupported));
    }
    if *token_program == TOKEN_PROGRAM_LEGACY && data.len() != MINT_BASE_LEN {
        return Err(fusion_err(FusionError::InvalidAccountOrder));
    }

    require!(data[45] == 1, FusionError::InvalidAccountOrder);
    Ok(data[44])
}

fn validate_token_account(
    token_account: &AccountView,
    token_program: &Address,
    expected_mint: &Address,
    expected_owner: &Address,
) -> ProgramResult {
    require_eq!(token_account.owner(), token_program, FusionError::InvalidAccountOrder);
    let data = token_account.try_borrow()?;
    require!(
        data.len() >= TOKEN_ACCOUNT_BASE_LEN,
        FusionError::InvalidAccountOrder
    );

    let mint = address_from_slice(&data[0..32])?;
    let owner = address_from_slice(&data[32..64])?;
    require_eq!(&mint, expected_mint, FusionError::InvalidAccountOrder);
    require_eq!(&owner, expected_owner, FusionError::InvalidAccountOrder);

    // 0 = uninitialized, 1 = initialized, 2 = frozen.
    require!(data[108] != 0, FusionError::InvalidAccountOrder);
    Ok(())
}

fn load_market(market_ai: &AccountView) -> Result<Market> {
    require!(market_ai.owned_by(&id()), FusionError::InvalidAccountOrder);

    let data = market_ai.try_borrow()?;
    require!(data.len() >= 8, FusionError::InvalidAccountOrder);
    require_eq!(&data[..8], &MARKET_DISCRIMINATOR, FusionError::InvalidAccountOrder);

    let mut bytes = &data[8..];
    Market::deserialize(&mut bytes).map_err(|_| fusion_err(FusionError::InvalidAccountOrder))
}

fn store_market(accounts: &mut [AccountView], market_idx: usize, market: &Market) -> ProgramResult {
    let market_ai = &mut accounts[market_idx];
    require!(market_ai.owned_by(&id()), FusionError::InvalidAccountOrder);
    let encoded = market
        .try_to_vec()
        .map_err(|_| fusion_err(FusionError::InvalidInstructionData))?;

    let mut data = market_ai.try_borrow_mut()?;
    require!(data.len() >= 8, FusionError::InvalidAccountOrder);
    require!(data.len() >= 8 + encoded.len(), FusionError::InvalidAccountOrder);

    data[..8].copy_from_slice(&MARKET_DISCRIMINATOR);
    data[8..8 + encoded.len()].copy_from_slice(&encoded);
    for b in &mut data[8 + encoded.len()..] {
        *b = 0;
    }
    Ok(())
}

fn addr_to_pubkey(address: &Address) -> solana_program::pubkey::Pubkey {
    solana_program::pubkey::Pubkey::new_from_array(address.to_bytes())
}

fn address_from_slice(slice: &[u8]) -> Result<Address> {
    require_eq!(slice.len(), 32, FusionError::InvalidAccountOrder);
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(slice);
    Ok(Address::from(bytes))
}

fn cancel_order_internal(market: &mut Market, owner: solana_program::pubkey::Pubkey, order_id: u64) -> Result<()> {
    let idx = market.find_order_index(order_id)?;
    let snapshot = market.orders[to_usize(idx)?];
    require_eq!(snapshot.owner, owner, FusionError::Unauthorized);
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
    owner: solana_program::pubkey::Pubkey,
) -> Result<(u64, u64, bool)> {
    let order = market.order_mut(idx)?;
    require_eq!(order.owner, owner, FusionError::Unauthorized);

    let base_out = order
        .base_claimable
        .checked_add(if order.open { 0 } else { order.locked_base })
        .ok_or(fusion_err(FusionError::MathOverflow))?;
    let quote_out = order
        .quote_claimable
        .checked_add(if order.open { 0 } else { order.locked_quote })
        .ok_or(fusion_err(FusionError::MathOverflow))?;

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

#[derive(BorshSerialize, BorshDeserialize, Clone, Copy)]
pub struct Market {
    pub admin: solana_program::pubkey::Pubkey,
    pub base_mint: solana_program::pubkey::Pubkey,
    pub quote_mint: solana_program::pubkey::Pubkey,
    pub base_vault: solana_program::pubkey::Pubkey,
    pub quote_vault: solana_program::pubkey::Pubkey,
    pub bump: u8,
    pub vault_authority_bump: u8,
    pub bids_head: i16,
    pub asks_head: i16,
    pub free_head: i16,
    pub next_order_id: u64,
    pub order_count: u16,
    pub orders: [OrderNode; MAX_ORDERS],
}

impl Default for Market {
    fn default() -> Self {
        Self {
            admin: solana_program::pubkey::Pubkey::default(),
            base_mint: solana_program::pubkey::Pubkey::default(),
            quote_mint: solana_program::pubkey::Pubkey::default(),
            base_vault: solana_program::pubkey::Pubkey::default(),
            quote_vault: solana_program::pubkey::Pubkey::default(),
            bump: 0,
            vault_authority_bump: 0,
            bids_head: NONE_INDEX,
            asks_head: NONE_INDEX,
            free_head: NONE_INDEX,
            next_order_id: 0,
            order_count: 0,
            orders: [OrderNode::default(); MAX_ORDERS],
        }
    }
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
            .ok_or(fusion_err(FusionError::MathOverflow))?;
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
        Err(fusion_err(FusionError::OrderNotFound))
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

fn map_engine_error(err: fusion_engine::FusionEngineError) -> ProgramError {
    match err {
        fusion_engine::FusionEngineError::InvalidSide => fusion_err(FusionError::InvalidSide),
        fusion_engine::FusionEngineError::InvalidInstructionTag
        | fusion_engine::FusionEngineError::InvalidInstructionData => {
            fusion_err(FusionError::InvalidInstructionData)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_test_market() -> Market {
        let mut market = Market {
            admin: solana_program::pubkey::Pubkey::new_unique(),
            base_mint: solana_program::pubkey::Pubkey::new_unique(),
            quote_mint: solana_program::pubkey::Pubkey::new_unique(),
            base_vault: solana_program::pubkey::Pubkey::new_unique(),
            quote_vault: solana_program::pubkey::Pubkey::new_unique(),
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
        user: solana_program::pubkey::Pubkey,
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
                    .ok_or(fusion_err(FusionError::MathOverflow))?;
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
                .ok_or(fusion_err(FusionError::MathOverflow))?;

            {
                let maker_mut = market.order_mut(best_idx)?;
                maker_mut.qty = maker_mut
                    .qty
                    .checked_sub(trade_qty)
                    .ok_or(fusion_err(FusionError::MathOverflow))?;

                match side {
                    Side::Bid => {
                        maker_mut.locked_base = maker_mut
                            .locked_base
                            .checked_sub(trade_qty)
                            .ok_or(fusion_err(FusionError::MathOverflow))?;
                        maker_mut.quote_claimable = maker_mut
                            .quote_claimable
                            .checked_add(trade_quote)
                            .ok_or(fusion_err(FusionError::MathOverflow))?;
                        taker_locked_quote = taker_locked_quote
                            .checked_sub(trade_quote)
                            .ok_or(fusion_err(FusionError::MathOverflow))?;
                    }
                    Side::Ask => {
                        maker_mut.locked_quote = maker_mut
                            .locked_quote
                            .checked_sub(trade_quote)
                            .ok_or(fusion_err(FusionError::MathOverflow))?;
                        maker_mut.base_claimable = maker_mut
                            .base_claimable
                            .checked_add(trade_qty)
                            .ok_or(fusion_err(FusionError::MathOverflow))?;
                        taker_locked_base = taker_locked_base
                            .checked_sub(trade_qty)
                            .ok_or(fusion_err(FusionError::MathOverflow))?;
                    }
                }
            }

            quantity = quantity
                .checked_sub(trade_qty)
                .ok_or(fusion_err(FusionError::MathOverflow))?;

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
            .ok_or(fusion_err(FusionError::MathOverflow))?;

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
                    .ok_or(fusion_err(FusionError::MathOverflow))?;
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
            .ok_or(fusion_err(FusionError::MathOverflow))?;

        Ok(Some(order_id))
    }

    #[test]
    fn partial_fill_then_resting_maker_state_is_preserved() {
        let mut market = new_test_market();
        let maker = solana_program::pubkey::Pubkey::new_unique();
        let taker = solana_program::pubkey::Pubkey::new_unique();

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
        let maker = solana_program::pubkey::Pubkey::new_unique();
        let taker = solana_program::pubkey::Pubkey::new_unique();

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
        let maker = solana_program::pubkey::Pubkey::new_unique();
        let taker = solana_program::pubkey::Pubkey::new_unique();

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
