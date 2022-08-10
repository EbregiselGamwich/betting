use anchor_lang::prelude::*;

pub const OPERATOR_PUBKEY: Pubkey = Pubkey::new_from_array([
    34, 72, 149, 62, 248, 255, 6, 27, 196, 250, 44, 189, 21, 35, 70, 134, 103, 80, 185, 50, 9, 76, 168, 111, 226, 48,
    58, 221, 46, 143, 217, 96,
]);

pub const RENT_PER_POSITION: u64 = 1447680;
pub const RENT_PER_BET: u64 = 1224960;
pub const RENT_PER_ORACLE: u64 = 1183200;

pub const MIN_BET_AMOUNT: u64 = 1000000 / 100;
pub const MIN_ORACLE_STAKE: u64 = 1000000;
