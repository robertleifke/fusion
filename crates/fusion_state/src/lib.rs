use solana_program::pubkey::Pubkey;

pub const MAX_ORDERS: usize = 128;
pub const NONE_INDEX: i16 = -1;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Bid = 0,
    Ask = 1,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OrderId(pub u64);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MarketKeys {
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
}
