use anchor_lang::prelude::*;

use crate::{constants::ORACLE_UPDATE_WINDOW, error::BettingError, state::Book};

#[derive(Accounts)]
pub struct BookMatchBetsAccounts<'info> {
    #[account(mut,seeds=[b"Book".as_ref(),&book_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
}

pub fn book_match_bets(ctx: Context<BookMatchBetsAccounts>) -> Result<()> {
    // check window
    let now = Clock::get()?.unix_timestamp;
    require!(
        ctx.accounts.book_pda.concluded_at.is_none()
            || ctx.accounts.book_pda.concluded_at.unwrap() + ORACLE_UPDATE_WINDOW > now,
        BettingError::NotInWindow
    );
    // match bets
    while ctx.accounts.book_pda.match_bets() {}
    Ok(())
}

#[cfg(test)]
mod test {
    use std::{
        collections::{BTreeMap, VecDeque},
        rc::Rc,
    };

    use anchor_client::RequestBuilder;
    use anchor_lang::{AccountDeserialize, AccountSerialize, AnchorSerialize};
    use solana_program_test::{tokio, ProgramTest};
    use solana_sdk::{
        account::Account, pubkey::Pubkey, rent::Rent, signature::Keypair, signer::Signer, transaction::Transaction,
    };

    use crate::state::{BetDirection, BetType, Book, Position};

    #[tokio::test]
    async fn test_book_match_bets_success() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let game_id: u32 = 1;
        let bet_type = BetType::One { handicap: 0 };
        let (book_pda, _) = Pubkey::find_program_address(
            &[
                b"Book".as_ref(),
                &game_id.to_le_bytes(),
                bet_type.try_to_vec().unwrap().as_slice(),
            ],
            &program_id,
        );
        let mut book_pda_state = Book {
            total_oracle_stake: 0,
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: None,
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: None,
        };
        let bettor_key = Pubkey::new_unique();
        book_pda_state.positions.insert(
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
        book_pda_state.new_bet(1200, 1000000 * 100, bettor_key, BetDirection::For);
        book_pda_state.new_bet(1200, 1000000 * 100, bettor_key, BetDirection::For);
        book_pda_state.new_bet(6000, 1000000 * 200, bettor_key, BetDirection::Against);
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        program_test.add_account(
            book_pda,
            Account {
                lamports: Rent::default().minimum_balance(book_pda_state.current_space()),
                data: book_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

        let rb = RequestBuilder::from(
            program_id,
            "",
            Rc::new(Keypair::new()),
            None,
            anchor_client::RequestNamespace::Global,
        );
        let instructions = rb
            .accounts(crate::accounts::BookMatchBetsAccounts { book_pda })
            .args(crate::instruction::BookMatchBets)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // the bets should be matched
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.bets_count, 3);
        assert_eq!(book_state.wager_total, 1000000 * 400);
        assert_eq!(book_state.payout_for_total, 1000000 * 240);
        assert_eq!(book_state.payout_against_total, 1000000 * 240);
        assert_eq!(book_state.dealt_wager, 1000000 * 240);
        assert!(book_state.bets_for.is_empty());
        assert_eq!(book_state.bets_against.len(), 1);
        assert_eq!(book_state.bets_against[0].wager, 1000000 * 160);
        assert_eq!(book_state.positions[&bettor_key].active_bets_count, 1);
        assert_eq!(book_state.positions[&bettor_key].bets_count, 3);
        assert_eq!(book_state.positions[&bettor_key].payout_for, 1000000 * 240);
        assert_eq!(book_state.positions[&bettor_key].payout_against, 1000000 * 240);
        assert_eq!(book_state.positions[&bettor_key].wager, 1000000 * 400);
        assert_eq!(book_state.positions[&bettor_key].dealt_wager, 1000000 * 240);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_match_bets_err_match_after_conclusion() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let game_id: u32 = 1;
        let bet_type = BetType::One { handicap: 0 };
        let (book_pda, _) = Pubkey::find_program_address(
            &[
                b"Book".as_ref(),
                &game_id.to_le_bytes(),
                bet_type.try_to_vec().unwrap().as_slice(),
            ],
            &program_id,
        );
        let mut book_pda_state = Book {
            total_oracle_stake: 0,
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: None,
        };
        let bettor_key = Pubkey::new_unique();
        book_pda_state.positions.insert(
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
        book_pda_state.new_bet(1200, 1000000 * 100, bettor_key, BetDirection::For);
        book_pda_state.new_bet(1200, 1000000 * 100, bettor_key, BetDirection::For);
        book_pda_state.new_bet(6000, 1000000 * 200, bettor_key, BetDirection::Against);
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        program_test.add_account(
            book_pda,
            Account {
                lamports: Rent::default().minimum_balance(book_pda_state.current_space()),
                data: book_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

        let rb = RequestBuilder::from(
            program_id,
            "",
            Rc::new(Keypair::new()),
            None,
            anchor_client::RequestNamespace::Global,
        );
        let instructions = rb
            .accounts(crate::accounts::BookMatchBetsAccounts { book_pda })
            .args(crate::instruction::BookMatchBets)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // the bets should be matched
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.bets_count, 3);
        assert_eq!(book_state.wager_total, 1000000 * 400);
        assert_eq!(book_state.payout_for_total, 1000000 * 240);
        assert_eq!(book_state.payout_against_total, 1000000 * 240);
        assert_eq!(book_state.dealt_wager, 1000000 * 240);
        assert!(book_state.bets_for.is_empty());
        assert_eq!(book_state.bets_against.len(), 1);
        assert_eq!(book_state.bets_against[0].wager, 1000000 * 160);
        assert_eq!(book_state.positions[&bettor_key].active_bets_count, 1);
        assert_eq!(book_state.positions[&bettor_key].bets_count, 3);
        assert_eq!(book_state.positions[&bettor_key].payout_for, 1000000 * 240);
        assert_eq!(book_state.positions[&bettor_key].payout_against, 1000000 * 240);
        assert_eq!(book_state.positions[&bettor_key].wager, 1000000 * 400);
        assert_eq!(book_state.positions[&bettor_key].dealt_wager, 1000000 * 240);
    }
}
