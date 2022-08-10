use anchor_lang::prelude::*;

use crate::{
    constants::{BETTOR_DISPUTE_WINDOW, OPERATOR_PUBKEY},
    error::BettingError,
    state::{BetOutcome, Book},
};

#[derive(Accounts)]
pub struct BookOperatorResolveDisputeAccounts<'info> {
    #[account(address=OPERATOR_PUBKEY)]
    pub operator: Signer<'info>,
    #[account(mut,seeds=[b"Book".as_ref(),&book_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
}

pub fn book_operator_resolve_dispute(
    ctx: Context<BookOperatorResolveDisputeAccounts>,
    bet_outcome: BetOutcome,
) -> Result<()> {
    // check if in dispute
    require!(ctx.accounts.book_pda.total_dispute_stake > 0, BettingError::NoAuthority);
    // check if concluded
    require!(ctx.accounts.book_pda.concluded_at.is_some(), BettingError::NoAuthority);
    // check if dispute window passed
    let now = Clock::get()?.unix_timestamp;
    let concluded_at = ctx.accounts.book_pda.concluded_at.unwrap();
    require!(concluded_at + BETTOR_DISPUTE_WINDOW < now, BettingError::NotInWindow);
    // update book pda
    ctx.accounts.book_pda.dispute_resolution_result = Some(bet_outcome);

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
    use home::home_dir;
    use solana_program_test::{tokio, ProgramTest};
    use solana_sdk::{
        account::Account,
        native_token::LAMPORTS_PER_SOL,
        pubkey::Pubkey,
        rent::Rent,
        signature::{read_keypair_file, Keypair},
        signer::Signer,
        transaction::Transaction,
    };

    use crate::{
        constants::BETTOR_DISPUTE_WINDOW,
        state::{BetOutcome, BetType, Book},
    };

    #[tokio::test]
    async fn test_book_operator_resolve_dispute_success() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let key_file_path = home_dir().unwrap().join(".config/solana/id.json");
        let operator = read_keypair_file(key_file_path).unwrap();
        program_test.add_account(
            operator.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let game_id: u32 = 2;
        let bet_type = BetType::One { handicap: 0 };
        let (book_pda, _) = Pubkey::find_program_address(
            &[
                b"Book".as_ref(),
                &game_id.to_le_bytes(),
                bet_type.try_to_vec().unwrap().as_slice(),
            ],
            &program_id,
        );
        let book_pda_state = Book {
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 1000000 * 10,
            dispute_resolution_result: None,
            concluded_at: Some(chrono::Utc::now().timestamp() - BETTOR_DISPUTE_WINDOW - 30),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: Rent::default().minimum_balance(book_pda_data.len()),
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
            .signer(&operator)
            .accounts(crate::accounts::BookOperatorResolveDisputeAccounts {
                operator: operator.pubkey(),
                book_pda,
            })
            .args(crate::instruction::BookOperatorResolveDispute {
                bet_outcome: BetOutcome::For,
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &operator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.dispute_resolution_result, Some(BetOutcome::For));
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6000)")]
    async fn test_book_operator_resolve_dispute_err_not_in_dispute() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let key_file_path = home_dir().unwrap().join(".config/solana/id.json");
        let operator = read_keypair_file(key_file_path).unwrap();
        program_test.add_account(
            operator.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let game_id: u32 = 2;
        let bet_type = BetType::One { handicap: 0 };
        let (book_pda, _) = Pubkey::find_program_address(
            &[
                b"Book".as_ref(),
                &game_id.to_le_bytes(),
                bet_type.try_to_vec().unwrap().as_slice(),
            ],
            &program_id,
        );
        let book_pda_state = Book {
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
            concluded_at: Some(chrono::Utc::now().timestamp() - BETTOR_DISPUTE_WINDOW - 30),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: Rent::default().minimum_balance(book_pda_data.len()),
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
            .signer(&operator)
            .accounts(crate::accounts::BookOperatorResolveDisputeAccounts {
                operator: operator.pubkey(),
                book_pda,
            })
            .args(crate::instruction::BookOperatorResolveDispute {
                bet_outcome: BetOutcome::For,
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &operator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.dispute_resolution_result, Some(BetOutcome::For));
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6000)")]
    async fn test_book_operator_resolve_dispute_not_concluded() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let key_file_path = home_dir().unwrap().join(".config/solana/id.json");
        let operator = read_keypair_file(key_file_path).unwrap();
        program_test.add_account(
            operator.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let game_id: u32 = 2;
        let bet_type = BetType::One { handicap: 0 };
        let (book_pda, _) = Pubkey::find_program_address(
            &[
                b"Book".as_ref(),
                &game_id.to_le_bytes(),
                bet_type.try_to_vec().unwrap().as_slice(),
            ],
            &program_id,
        );
        let book_pda_state = Book {
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 1000000 * 10,
            dispute_resolution_result: None,
            concluded_at: None,
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: Rent::default().minimum_balance(book_pda_data.len()),
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
            .signer(&operator)
            .accounts(crate::accounts::BookOperatorResolveDisputeAccounts {
                operator: operator.pubkey(),
                book_pda,
            })
            .args(crate::instruction::BookOperatorResolveDispute {
                bet_outcome: BetOutcome::For,
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &operator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.dispute_resolution_result, Some(BetOutcome::For));
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_operator_resolve_dispute_err_dispute_window_not_passed() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let key_file_path = home_dir().unwrap().join(".config/solana/id.json");
        let operator = read_keypair_file(key_file_path).unwrap();
        program_test.add_account(
            operator.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let game_id: u32 = 2;
        let bet_type = BetType::One { handicap: 0 };
        let (book_pda, _) = Pubkey::find_program_address(
            &[
                b"Book".as_ref(),
                &game_id.to_le_bytes(),
                bet_type.try_to_vec().unwrap().as_slice(),
            ],
            &program_id,
        );
        let book_pda_state = Book {
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 1000000 * 10,
            dispute_resolution_result: None,
            concluded_at: Some(chrono::Utc::now().timestamp() - BETTOR_DISPUTE_WINDOW + 60),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: Rent::default().minimum_balance(book_pda_data.len()),
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
            .signer(&operator)
            .accounts(crate::accounts::BookOperatorResolveDisputeAccounts {
                operator: operator.pubkey(),
                book_pda,
            })
            .args(crate::instruction::BookOperatorResolveDispute {
                bet_outcome: BetOutcome::For,
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &operator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.dispute_resolution_result, Some(BetOutcome::For));
    }
}
