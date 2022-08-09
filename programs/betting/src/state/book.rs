use std::collections::{BTreeMap, VecDeque};

use anchor_lang::prelude::*;

#[account]
pub struct Book {
    pub game_id: u32,
    pub initiator: Pubkey,
    pub bets_count: u32,
    pub wager_total: u64,
    pub payout_for_total: u64,
    pub payout_against_total: u64,
    pub dealt_wager: u64,
    pub bet_type: BetType,
    pub total_dispute_stake: u64,
    pub dispute_resolution_result: Option<BetOutcome>,
    pub concluded_at: Option<i64>,
    pub oracles: BTreeMap<Pubkey, Oracle>,
    pub bets_for: VecDeque<Bet>,
    pub bets_against: VecDeque<Bet>,
    pub positions: BTreeMap<Pubkey, Position>,
}
impl Book {
    pub const INIT_SPACE: usize =
        8 + 4 + 32 + 4 + 8 + 8 + 8 + 8 + BetType::INIT_SPACE + 8 + 1 + BetOutcome::INIT_SPACE + 1 + 8 + 4 + 4 + 4 + 4;
    pub fn current_space(&self) -> usize {
        Self::INIT_SPACE
            + (32 + Oracle::INIT_SPACE) * self.oracles.len()
            + Bet::INIT_SPACE * (self.bets_for.len() + self.bets_against.len())
            + (32 + Position::INIT_SPACE) * self.positions.len()
    }
    pub fn new_bet(&mut self, odds: u32, wager: u64, bettor: Pubkey, bet_direction: BetDirection) {
        let mut id = [0_u8; 8];
        id[0..4].copy_from_slice(self.bets_count.to_le_bytes().as_slice());
        id[4..8].copy_from_slice(odds.to_le_bytes().as_slice());
        let bet = Bet {
            id: u64::from_le_bytes(id),
            bettor,
            wager,
        };
        match bet_direction {
            BetDirection::For => match self.bets_for.binary_search_by_key(&bet.id, |b| b.id) {
                Ok(_) => {
                    panic!();
                }
                Err(index) => {
                    self.bets_for.insert(index, bet);
                }
            },
            BetDirection::Against => match self.bets_against.binary_search_by_key(&bet.id, |b| b.id) {
                Ok(_) => {
                    panic!();
                }
                Err(index) => {
                    self.bets_against.insert(index, bet);
                }
            },
        }

        self.bets_count += 1;
        self.wager_total += wager;
        let position = self.positions.get_mut(&bettor).unwrap();
        position.bets_count += 1;
        position.wager += wager;
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BetDirection {
    For,
    Against,
}
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct Oracle {
    pub stake: u64,
    pub outcome: Option<BetOutcome>,
}
impl Oracle {
    pub const INIT_SPACE: usize = 8 + 1 + BetOutcome::INIT_SPACE;
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BetType {
    One { handicap: i8 },
    X { handicap: i8 },
    Two { handicap: i8 },
}
impl BetType {
    pub const INIT_SPACE: usize = 1;
}
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub enum BetOutcome {
    For,
    Cancel,
    Against,
}
impl BetOutcome {
    pub const INIT_SPACE: usize = 1;
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct Bet {
    pub id: u64,
    pub bettor: Pubkey,
    pub wager: u64,
}

impl Bet {
    pub const INIT_SPACE: usize = 8 + 32 + 8;

    pub fn odds(&self) -> u32 {
        (self.id >> 32) as u32
    }
}
impl Ord for Bet {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}
impl PartialOrd for Bet {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.id.cmp(&other.id))
    }
}
impl PartialEq for Bet {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for Bet {}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct Position {
    pub active_bets_count: u32, // 4
    pub bets_count: u32,        // 4
    pub payout_for: u64,        // 8
    pub payout_against: u64,    // 8
    pub wager: u64,             // 8
    pub dealt_wager: u64,       // 8
    pub dispute_stake: u64,     // 8
}
impl Position {
    pub const INIT_SPACE: usize = 4 + 4 + 8 + 8 + 8 + 8 + 8;
}

#[cfg(test)]
mod test {
    use std::collections::{BTreeMap, VecDeque};

    use anchor_lang::AccountSerialize;
    use solana_sdk::pubkey::Pubkey;

    use crate::state::{BetDirection, BetOutcome, Oracle, Position};

    use super::{BetType, Book};

    #[test]
    fn test_state_book_init_space() {
        let book = Book {
            game_id: 1,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type: BetType::One { handicap: 0 },
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: None,
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
        };
        let mut book_data: Vec<u8> = Vec::new();
        book.try_serialize(&mut book_data).unwrap();
        assert!(book_data.len() <= Book::INIT_SPACE);
    }
    #[test]
    fn test_state_book_current_space() {
        let mut book = Book {
            game_id: 1,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type: BetType::One { handicap: 0 },
            total_dispute_stake: 0,
            dispute_resolution_result: Some(BetOutcome::For),
            concluded_at: Some(2),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
        };
        book.oracles.insert(
            Pubkey::new_unique(),
            Oracle {
                stake: 123,
                outcome: None,
            },
        );
        let bettor_key = Pubkey::new_unique();
        book.positions.insert(
            bettor_key,
            Position {
                active_bets_count: 0,
                bets_count: 0,
                payout_for: 0,
                payout_against: 0,
                wager: 0,
                dealt_wager: 0,
                dispute_stake: 0,
            },
        );
        book.new_bet(123, 123, bettor_key, BetDirection::For);
        let mut book_data: Vec<u8> = Vec::new();
        book.try_serialize(&mut book_data).unwrap();
        assert!(book_data.len() <= book.current_space());
    }
    #[test]
    fn test_state_book_new_bet() {
        let mut book = Book {
            game_id: 1,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type: BetType::One { handicap: 0 },
            total_dispute_stake: 0,
            dispute_resolution_result: Some(BetOutcome::For),
            concluded_at: Some(2),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
        };

        let bettor_key = Pubkey::new_unique();
        book.positions.insert(
            bettor_key,
            Position {
                active_bets_count: 0,
                bets_count: 0,
                payout_for: 0,
                payout_against: 0,
                wager: 0,
                dealt_wager: 0,
                dispute_stake: 0,
            },
        );
        book.new_bet(123, 123, bettor_key, BetDirection::For);

        assert_eq!(book.bets_count, 1);
        assert_eq!(book.wager_total, 123);
        assert_eq!(book.bets_for.len(), 1);
        assert_eq!(book.bets_for[0].odds(), 123);
        assert_eq!(book.bets_for[0].id, 528280977408);
        assert_eq!(book.positions[&bettor_key].bets_count, 1);
        assert_eq!(book.positions[&bettor_key].wager, 123);
    }
}
