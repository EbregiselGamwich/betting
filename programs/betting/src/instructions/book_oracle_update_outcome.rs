use anchor_lang::prelude::*;

use crate::{
    constants::ORACLE_UPDATE_WINDOW,
    error::BettingError,
    state::{BetOutcome, Book},
};

#[derive(Accounts)]
pub struct BookOracleUpdateOutcomeAccounts<'info> {
    pub oracle: Signer<'info>,
    #[account(mut,seeds=[b"Book".as_ref(),&book_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
}

pub fn book_oracle_update_outcome(
    ctx: Context<BookOracleUpdateOutcomeAccounts>,
    bet_outcome: Option<BetOutcome>,
) -> Result<()> {
    // check oracle update window
    let now = Clock::get()?.unix_timestamp;
    require!(
        ctx.accounts.book_pda.concluded_at.is_none()
            || ctx.accounts.book_pda.concluded_at.unwrap() + ORACLE_UPDATE_WINDOW > now,
        BettingError::NotInWindow
    );
    // update oracle
    match ctx.accounts.book_pda.oracles.get_mut(ctx.accounts.oracle.key) {
        Some(o) => {
            o.outcome = bet_outcome;
        }
        None => {
            return err!(BettingError::UserDidNotOptIn);
        }
    }
    // update book pda
    let aggregated_outcome = ctx.accounts.book_pda.aggregated_outcome();
    if ctx.accounts.book_pda.aggregated_oracle_outcome != aggregated_outcome {
        ctx.accounts.book_pda.aggregated_oracle_outcome = aggregated_outcome;
        if aggregated_outcome.is_some() {
            ctx.accounts.book_pda.concluded_at = Some(now);
        } else {
            ctx.accounts.book_pda.concluded_at = None;
        }
    }

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
        account::Account, native_token::LAMPORTS_PER_SOL, pubkey::Pubkey, rent::Rent, signature::Keypair,
        signer::Signer, transaction::Transaction,
    };

    use crate::state::{BetOutcome, BetType, Book, Oracle};

    #[tokio::test]
    async fn test_book_oracle_update_outcome_success() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let oracle = Keypair::new();
        program_test.add_account(
            oracle.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let game_id = 1_u32;
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
        book_pda_state.oracles.insert(
            oracle.pubkey(),
            Oracle {
                stake: 1000000 * 100,
                outcome: None,
            },
        );
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
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
            .signer(&oracle)
            .accounts(crate::accounts::BookOracleUpdateOutcomeAccounts {
                oracle: oracle.pubkey(),
                book_pda,
            })
            .args(crate::instruction::BookOracleUpdateOutcome {
                bet_outcome: Some(BetOutcome::For),
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &oracle],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.oracles[&oracle.pubkey()].outcome, Some(BetOutcome::For));
        assert!(book_state.concluded_at.is_some());
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_oracle_update_outcome_err_window_passed() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let oracle = Keypair::new();
        program_test.add_account(
            oracle.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let game_id = 1_u32;
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
        book_pda_state.oracles.insert(
            oracle.pubkey(),
            Oracle {
                stake: 1000000 * 100,
                outcome: None,
            },
        );
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
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
            .signer(&oracle)
            .accounts(crate::accounts::BookOracleUpdateOutcomeAccounts {
                oracle: oracle.pubkey(),
                book_pda,
            })
            .args(crate::instruction::BookOracleUpdateOutcome {
                bet_outcome: Some(BetOutcome::For),
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &oracle],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.oracles[&oracle.pubkey()].outcome, Some(BetOutcome::For));
        assert!(book_state.concluded_at.is_some());
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6004)")]
    async fn test_book_oracle_update_outcome_err_oracle() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let oracle = Keypair::new();
        program_test.add_account(
            oracle.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let game_id = 1_u32;
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
        book_pda_state.oracles.insert(
            Pubkey::new_unique(),
            Oracle {
                stake: 1000000 * 100,
                outcome: None,
            },
        );
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
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
            .signer(&oracle)
            .accounts(crate::accounts::BookOracleUpdateOutcomeAccounts {
                oracle: oracle.pubkey(),
                book_pda,
            })
            .args(crate::instruction::BookOracleUpdateOutcome {
                bet_outcome: Some(BetOutcome::For),
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &oracle],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.oracles[&oracle.pubkey()].outcome, Some(BetOutcome::For));
        assert!(book_state.concluded_at.is_some());
    }
}
