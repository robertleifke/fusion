use anchor_lang::prelude::{AnchorDeserialize, AnchorSerialize, Pubkey};

pub const MAX_ORDERS: usize = 128;
pub const NONE_INDEX: i16 = -1;
pub const ORDER_NODE_SIZE: usize = 96;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Side {
    Bid = 0,
    Ask = 1,
}

impl Side {
    pub fn opposite(&self) -> Side {
        match self {
            Side::Bid => Side::Ask,
            Side::Ask => Side::Bid,
        }
    }

    pub fn try_from_u8(v: u8) -> Result<Side, FusionEngineError> {
        match v {
            0 => Ok(Side::Bid),
            1 => Ok(Side::Ask),
            _ => Err(FusionEngineError::InvalidSide),
        }
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrderNode {
    pub used: bool,
    pub open: bool,
    pub in_book: bool,
    pub side: u8,
    pub next: i16,
    pub prev: i16,
    pub id: u64,
    pub owner: Pubkey,
    pub price: u64,
    pub qty: u64,
    pub locked_base: u64,
    pub locked_quote: u64,
    pub base_claimable: u64,
    pub quote_claimable: u64,
}

impl Default for OrderNode {
    fn default() -> Self {
        Self {
            used: false,
            open: false,
            in_book: false,
            side: Side::Bid as u8,
            next: NONE_INDEX,
            prev: NONE_INDEX,
            id: 0,
            owner: Pubkey::default(),
            price: 0,
            qty: 0,
            locked_base: 0,
            locked_quote: 0,
            base_claimable: 0,
            quote_claimable: 0,
        }
    }
}

impl OrderNode {
    pub const SIZE: usize = ORDER_NODE_SIZE;

    pub fn is_live(&self) -> bool {
        self.used && self.open && self.in_book && self.qty > 0
    }

    pub fn side_enum(&self) -> Result<Side, FusionEngineError> {
        Side::try_from_u8(self.side)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FusionInstruction {
    InitializeMarket(InitializeMarketArgs),
    PlaceOrder(PlaceOrderArgs),
    CancelOrder(CancelOrderArgs),
    ClaimOrderProceeds(ClaimOrderProceedsArgs),
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct InitializeMarketArgs {
    pub market_bump: u8,
    pub vault_authority_bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaceOrderArgs {
    pub side: u8,
    pub limit_price: u64,
    pub quantity: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct CancelOrderArgs {
    pub order_id: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimOrderProceedsArgs {
    pub order_id: u64,
}

impl FusionInstruction {
    pub const TAG_INITIALIZE_MARKET: u8 = 0;
    pub const TAG_PLACE_ORDER: u8 = 1;
    pub const TAG_CANCEL_ORDER: u8 = 2;
    pub const TAG_CLAIM_ORDER_PROCEEDS: u8 = 3;

    pub fn encode(&self) -> Result<Vec<u8>, FusionEngineError> {
        let mut out = Vec::with_capacity(64);
        match self {
            Self::InitializeMarket(args) => {
                out.push(Self::TAG_INITIALIZE_MARKET);
                out.extend_from_slice(
                    &args
                        .try_to_vec()
                        .map_err(|_| FusionEngineError::InvalidInstructionData)?,
                );
            }
            Self::PlaceOrder(args) => {
                out.push(Self::TAG_PLACE_ORDER);
                out.extend_from_slice(
                    &args
                        .try_to_vec()
                        .map_err(|_| FusionEngineError::InvalidInstructionData)?,
                );
            }
            Self::CancelOrder(args) => {
                out.push(Self::TAG_CANCEL_ORDER);
                out.extend_from_slice(
                    &args
                        .try_to_vec()
                        .map_err(|_| FusionEngineError::InvalidInstructionData)?,
                );
            }
            Self::ClaimOrderProceeds(args) => {
                out.push(Self::TAG_CLAIM_ORDER_PROCEEDS);
                out.extend_from_slice(
                    &args
                        .try_to_vec()
                        .map_err(|_| FusionEngineError::InvalidInstructionData)?,
                );
            }
        }
        Ok(out)
    }

    pub fn decode(data: &[u8]) -> Result<Self, FusionEngineError> {
        if data.is_empty() {
            return Err(FusionEngineError::InvalidInstructionData);
        }

        let (tag, payload) = data
            .split_first()
            .ok_or(FusionEngineError::InvalidInstructionData)?;

        match *tag {
            Self::TAG_INITIALIZE_MARKET => {
                let args = InitializeMarketArgs::try_from_slice(payload)
                    .map_err(|_| FusionEngineError::InvalidInstructionData)?;
                Ok(Self::InitializeMarket(args))
            }
            Self::TAG_PLACE_ORDER => {
                let args = PlaceOrderArgs::try_from_slice(payload)
                    .map_err(|_| FusionEngineError::InvalidInstructionData)?;
                Ok(Self::PlaceOrder(args))
            }
            Self::TAG_CANCEL_ORDER => {
                let args = CancelOrderArgs::try_from_slice(payload)
                    .map_err(|_| FusionEngineError::InvalidInstructionData)?;
                Ok(Self::CancelOrder(args))
            }
            Self::TAG_CLAIM_ORDER_PROCEEDS => {
                let args = ClaimOrderProceedsArgs::try_from_slice(payload)
                    .map_err(|_| FusionEngineError::InvalidInstructionData)?;
                Ok(Self::ClaimOrderProceeds(args))
            }
            _ => Err(FusionEngineError::InvalidInstructionTag),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FusionEngineError {
    InvalidSide,
    InvalidInstructionTag,
    InvalidInstructionData,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_order_instruction_roundtrip() {
        let ix = FusionInstruction::PlaceOrder(PlaceOrderArgs {
            side: Side::Bid as u8,
            limit_price: 42,
            quantity: 7,
        });

        let encoded = ix.encode().expect("encode should succeed");
        assert_eq!(encoded[0], FusionInstruction::TAG_PLACE_ORDER);

        let decoded = FusionInstruction::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, ix);
    }

    #[test]
    fn claim_instruction_golden_bytes() {
        let ix = FusionInstruction::ClaimOrderProceeds(ClaimOrderProceedsArgs { order_id: 0x0102_0304_0506_0708 });
        let encoded = ix.encode().expect("encode should succeed");

        // Tag + LE u64 order id.
        assert_eq!(hex::encode(encoded), "030807060504030201");
    }

    #[test]
    fn decode_rejects_unknown_tag() {
        let err = FusionInstruction::decode(&[255, 1, 2, 3]).expect_err("decode should fail");
        assert_eq!(err, FusionEngineError::InvalidInstructionTag);
    }
}
