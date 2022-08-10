use std::{
    cmp::Ordering,
    collections::{BTreeMap, VecDeque},
};

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
    pub fn aggregated_outcome(&self) -> Option<BetOutcome> {
        let mut map: BTreeMap<Option<BetOutcome>, u64> = BTreeMap::new();

        for o in self.oracles.values() {
            *map.entry(o.outcome).or_insert(0) += o.stake;
        }

        let mut vec = Vec::from_iter(map);
        vec.sort_by_key(|kv| kv.1);

        vec[vec.len() - 1].0
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
        position.active_bets_count += 1;
    }
    pub fn match_bets(&mut self) -> bool {
        let mut matched = false;
        if self.dealt_wager >= self.payout_for_total && self.dealt_wager >= self.payout_against_total {
            match self.payout_for_total.cmp(&self.payout_against_total) {
                Ordering::Equal => {
                    if !self.bets_for.is_empty()
                        && !self.bets_against.is_empty()
                        && self.bets_for.front().unwrap().odds_f() <= self.bets_against.front().unwrap().opposite_odds()
                    {
                        let mut bet_for = self.bets_for.pop_front().unwrap();
                        let mut bet_against = self.bets_against.pop_front().unwrap();
                        let mut against_payout = 0_u64;
                        let mut against_dealt_wager = 0_u64;
                        let mut for_payout = 0_u64;
                        let mut for_dealt_wager = 0_u64;
                        if bet_for.payout() <= bet_against.wager + bet_for.wager {
                            let against_wager = bet_for.payout() - bet_for.wager;

                            against_payout += bet_against.get_payout_by_wager(against_wager);
                            against_dealt_wager += against_wager;
                            bet_against.wager -= against_wager;

                            for_payout += bet_for.payout();
                            for_dealt_wager += bet_for.wager;
                            bet_for.wager = 0;

                            matched = true;
                        } else if bet_against.payout() <= bet_for.wager + bet_against.wager {
                            let for_wager = bet_against.payout() - bet_against.wager;

                            for_payout += bet_for.get_payout_by_wager(for_wager);
                            for_dealt_wager += for_wager;
                            bet_for.wager -= for_wager;

                            against_payout += bet_against.payout();
                            against_dealt_wager += bet_against.wager;
                            bet_against.wager = 0;

                            matched = true;
                        }
                        self.positions.entry(bet_for.bettor).and_modify(|p| {
                            p.payout_for += for_payout;
                            p.dealt_wager += for_dealt_wager;

                            self.payout_for_total += for_payout;
                            self.dealt_wager += for_dealt_wager;

                            if bet_for.wager > 0 {
                                self.bets_for.push_front(bet_for);
                            } else {
                                p.active_bets_count -= 1;
                            }
                        });
                        self.positions.entry(bet_against.bettor).and_modify(|p| {
                            p.payout_against += against_payout;
                            p.dealt_wager += against_dealt_wager;

                            self.payout_against_total += against_payout;
                            self.dealt_wager += against_dealt_wager;
                            if bet_against.wager > 0 {
                                self.bets_against.push_front(bet_against);
                            } else {
                                p.active_bets_count -= 1;
                            }
                        });
                    }
                }
                Ordering::Greater => {
                    if let Some(mut bet_against) = self.bets_against.pop_front() {
                        let position_against = self.positions.get_mut(&bet_against.bettor).unwrap();
                        let payout_diff = self.payout_for_total - self.payout_against_total;

                        position_against.payout_against += payout_diff;
                        let dealt_wager = bet_against.get_wager_by_payout(payout_diff);
                        position_against.dealt_wager += dealt_wager;
                        bet_against.wager -= dealt_wager;

                        self.payout_against_total += payout_diff;
                        self.dealt_wager += dealt_wager;

                        matched = true;

                        if bet_against.wager > 0 {
                            self.bets_against.push_front(bet_against)
                        } else {
                            position_against.active_bets_count -= 1;
                        }
                    }
                }
                Ordering::Less => {
                    if let Some(mut bet_for) = self.bets_for.pop_front() {
                        let position_for = self.positions.get_mut(&bet_for.bettor).unwrap();
                        let payout_diff = self.payout_against_total - self.payout_for_total;

                        position_for.payout_for += payout_diff;
                        let dealt_wager = bet_for.get_wager_by_payout(payout_diff);
                        position_for.dealt_wager += dealt_wager;
                        bet_for.wager -= dealt_wager;

                        self.payout_for_total += payout_diff;
                        self.dealt_wager += dealt_wager;

                        matched = true;

                        if bet_for.wager > 0 {
                            self.bets_for.push_front(bet_for);
                        } else {
                            position_for.active_bets_count -= 1;
                        }
                    }
                }
            }
        }
        matched
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
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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

    pub fn odds_f(&self) -> f64 {
        (self.odds() as f64) / 1000.0
    }

    pub fn opposite_odds(&self) -> f64 {
        1.0 / (self.odds_f() - 1.0) + 1.0
    }
    pub fn payout(&self) -> u64 {
        ((self.wager as f64) * self.odds_f()).floor() as u64
    }
    pub fn get_payout_by_wager(&self, wager: u64) -> u64 {
        ((wager as f64) * self.odds_f()).floor() as u64
    }
    pub fn get_wager_by_payout(&self, payout: u64) -> u64 {
        (payout as f64 / self.odds_f()).ceil() as u64
    }
}

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
