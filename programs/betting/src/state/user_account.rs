use std::collections::{BTreeMap, BTreeSet};

use anchor_lang::prelude::*;

#[account]
pub struct UserAccount {
    pub authority: Pubkey,
    pub books_initialized: u32,
    pub books_oracled: BTreeSet<Pubkey>,
    pub bets: BTreeMap<Pubkey, BTreeSet<u64>>,
}
impl UserAccount {
    pub const INIT_SPACE: usize = 8 + 32 + 4 + 4 + 4;
    pub fn current_space(&self) -> usize {
        let mut space = Self::INIT_SPACE;
        space += self.books_oracled.len() * 32;
        for v in self.bets.values() {
            space = space + 32 + 4 + 8 * (v.len());
        }
        space
    }
}

#[cfg(test)]
mod test {
    use std::collections::{BTreeMap, BTreeSet};

    use anchor_lang::AccountSerialize;
    use solana_sdk::pubkey::Pubkey;

    use super::UserAccount;

    #[test]
    fn test_init_space() {
        let ua = UserAccount {
            authority: Pubkey::new_unique(),
            bets: BTreeMap::new(),
            books_initialized: 0,
            books_oracled: BTreeSet::new(),
        };
        let mut data: Vec<u8> = Vec::new();
        ua.try_serialize(&mut data).unwrap();
        assert_eq!(data.len(), UserAccount::INIT_SPACE);
    }
    #[test]
    fn test_current_space() {
        let mut ua = UserAccount {
            authority: Pubkey::new_unique(),
            bets: BTreeMap::new(),
            books_initialized: 12,
            books_oracled: BTreeSet::from_iter(vec![Pubkey::new_unique()]),
        };
        let addr1 = Pubkey::new_unique();
        let addr2 = Pubkey::new_unique();
        ua.bets.entry(addr1).or_insert(BTreeSet::new()).insert(1);
        ua.bets.entry(addr1).or_insert(BTreeSet::new()).insert(2);
        ua.bets.entry(addr1).or_insert(BTreeSet::new()).insert(3);

        ua.bets.entry(addr2).or_insert(BTreeSet::new()).insert(1);
        ua.bets.entry(addr2).or_insert(BTreeSet::new()).insert(2);

        let mut data: Vec<u8> = Vec::new();
        ua.try_serialize(&mut data).unwrap();
        assert_eq!(data.len(), ua.current_space());
    }
}
