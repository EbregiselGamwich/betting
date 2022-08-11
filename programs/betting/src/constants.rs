use anchor_lang::prelude::*;

pub const OPERATOR_PUBKEY: Pubkey = Pubkey::new_from_array([
    34, 72, 149, 62, 248, 255, 6, 27, 196, 250, 44, 189, 21, 35, 70, 134, 103, 80, 185, 50, 9, 76, 168, 111, 226, 48,
    58, 221, 46, 143, 217, 96,
]);

pub const OPERATOR_TOKEN_ACCOUNT: Pubkey = Pubkey::new_from_array([
    0x0f, 0x05, 0xb2, 0x61, 0xfc, 0xb6, 0x32, 0xb9, 0x55, 0x77, 0x97, 0xfa, 0xeb, 0xb8, 0xa5, 0xbc, 0x10, 0x0b, 0x8f,
    0xc5, 0x40, 0xf0, 0x21, 0x7c, 0x74, 0x70, 0xd8, 0x6e, 0xe6, 0xc5, 0xdc, 0x3e,
]);

pub const RENT_PER_POSITION: u64 = 1447680;
pub const RENT_PER_BET: u64 = 1224960;
pub const RENT_PER_ORACLE: u64 = 1183200;

pub const MIN_BET_AMOUNT: u64 = 1000000 / 100;
pub const MIN_ORACLE_STAKE: u64 = 1000000;
pub const MIN_BETTOR_DISPUTE_STAKE: u64 = 1000000 * 10;

pub const ORACLE_UPDATE_WINDOW: i64 = 60 * 10;
pub const BETTOR_DISPUTE_WINDOW: i64 = ORACLE_UPDATE_WINDOW + 60 * 20;

pub const BETTOR_PAYOUT_RATE: u64 = 9900;

pub const ORALCES_REWARD_SHARE: u64 = 6000;
pub const INITIATOR_REWARD_SHARE: u64 = 2000;
