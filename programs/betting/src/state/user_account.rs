use std::collections::VecDeque;

use anchor_lang::prelude::*;

#[account]
pub struct UserAccount {
    pub authority: Pubkey,
    pub books_initialized: u32,
    pub books_oracled: VecDeque<Pubkey>,
    pub books_bet_on: VecDeque<Pubkey>,
}
impl UserAccount {
    pub const INIT_SPACE: usize = 8 + 32 + 4 + 4 + 4;
    pub fn current_space(&self) -> usize {
        Self::INIT_SPACE + 32 * (self.books_oracled.len() + self.books_bet_on.len())
    }
}

#[cfg(test)]
mod test {
    use std::collections::VecDeque;

    use anchor_lang::AccountSerialize;
    use solana_sdk::pubkey::Pubkey;

    use super::UserAccount;

    #[test]
    fn test_init_space() {
        let ua = UserAccount {
            authority: Pubkey::new_unique(),
            books_bet_on: VecDeque::new(),
            books_initialized: 0,
            books_oracled: VecDeque::new(),
        };
        let mut data: Vec<u8> = Vec::new();
        ua.try_serialize(&mut data).unwrap();
        assert_eq!(data.len(), UserAccount::INIT_SPACE);
    }
    #[test]
    fn test_current_space() {
        let mut ua = UserAccount {
            authority: Pubkey::new_unique(),
            books_bet_on: VecDeque::new(),
            books_initialized: 12,
            books_oracled: VecDeque::new(),
        };

        ua.books_bet_on.insert(0, Pubkey::new_unique());
        ua.books_oracled.insert(0, Pubkey::new_unique());

        let mut data: Vec<u8> = Vec::new();
        ua.try_serialize(&mut data).unwrap();
        assert_eq!(data.len(), ua.current_space());
    }
}
