use anchor_lang::{prelude::*, system_program};

use crate::{
    constants::{ORACLE_UPDATE_WINDOW, RENT_PER_POSITION},
    error::BettingError,
    state::{user_account::UserAccount, Book, Position},
};

#[derive(Accounts)]
pub struct BookBettorOptInAccounts<'info> {
    #[account(mut)]
    pub bettor: Signer<'info>,
    #[account(mut,seeds=[b"UserAccount".as_ref(),bettor.key().as_ref()],bump)]
    pub bettor_user_account: Account<'info, UserAccount>,
    #[account(mut,seeds=[b"Book".as_ref(),&book_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
    pub system_program: Program<'info, System>,
}

pub fn book_bettor_opt_int(ctx: Context<BookBettorOptInAccounts>) -> Result<()> {
    // check window
    let now = Clock::get()?.unix_timestamp;
    require!(
        ctx.accounts.book_pda.concluded_at.is_none()
            || ctx.accounts.book_pda.concluded_at.unwrap() + ORACLE_UPDATE_WINDOW > now,
        BettingError::NotInWindow
    );
    // update bettor user account
    match ctx
        .accounts
        .bettor_user_account
        .books_bet_on
        .binary_search(&ctx.accounts.book_pda.key())
    {
        Ok(_) => {
            return err!(BettingError::UserAlreadyOptIn);
        }
        Err(index) => {
            ctx.accounts
                .bettor_user_account
                .books_bet_on
                .insert(index, ctx.accounts.book_pda.key());
            let bettor_user_account_info = ctx.accounts.bettor_user_account.to_account_info();
            let user_account_space = ctx.accounts.bettor_user_account.current_space();
            let user_account_min_rent = Rent::get()?.minimum_balance(user_account_space);
            bettor_user_account_info.realloc(user_account_space, false)?;
            if bettor_user_account_info.lamports() < user_account_min_rent {
                let diff = user_account_min_rent - bettor_user_account_info.lamports();
                let user_account_rent_transfer_cpi_context = CpiContext::new(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.bettor.to_account_info(),
                        to: ctx.accounts.bettor_user_account.to_account_info(),
                    },
                );
                system_program::transfer(user_account_rent_transfer_cpi_context, diff)?;
            }
        }
    }

    // update book pda
    if ctx.accounts.book_pda.positions.contains_key(ctx.accounts.bettor.key) {
        return err!(BettingError::UserAlreadyOptIn);
    } else {
        // create position
        ctx.accounts.book_pda.positions.insert(
            ctx.accounts.bettor.key(),
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
        // realloc
        let book_pda_account_info = ctx.accounts.book_pda.to_account_info();
        let book_pda_space = ctx.accounts.book_pda.current_space();
        book_pda_account_info.realloc(book_pda_space, false)?;

        // transfer rent
        let position_rent_transfer_cpi_context = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            system_program::Transfer {
                from: ctx.accounts.bettor.to_account_info(),
                to: ctx.accounts.book_pda.to_account_info(),
            },
        );
        system_program::transfer(position_rent_transfer_cpi_context, RENT_PER_POSITION)?;
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
        signer::Signer, system_program, transaction::Transaction,
    };

    use crate::state::{user_account::UserAccount, BetType, Book, Position};

    #[tokio::test]
    async fn test_book_bettor_opt_in_success() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let bettor = Keypair::new();
        program_test.add_account(
            bettor.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let (bettor_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), bettor.pubkey().as_ref()], &program_id);
        let bettor_pda_state = UserAccount {
            authority: bettor.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::new(),
            books_bet_on: VecDeque::new(),
        };
        let mut bettor_pda_data: Vec<u8> = Vec::new();
        bettor_pda_state.try_serialize(&mut bettor_pda_data).unwrap();
        program_test.add_account(
            bettor_pda,
            Account {
                lamports: Rent::default().minimum_balance(bettor_pda_state.current_space()),
                data: bettor_pda_data,
                owner: program_id,
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
        let book_pda_state = Book {
            total_oracle_stake: 0,
            aggregated_oracle_outcome: None,
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
        };
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
            .signer(&bettor)
            .accounts(crate::accounts::BookBettorOptInAccounts {
                bettor: bettor.pubkey(),
                bettor_user_account: bettor_pda,
                book_pda,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorOptInt)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &bettor],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be added to the bettor user account
        let bettor_user_account = banks_client.get_account(bettor_pda).await.unwrap().unwrap();
        let bettor_user_account_state = UserAccount::try_deserialize(&mut bettor_user_account.data.as_slice()).unwrap();
        assert!(bettor_user_account_state.books_bet_on.contains(&book_pda));
        // a position should be created in the book pda account
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.positions.contains_key(&bettor.pubkey()));
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6003")] // already opt in
    async fn test_book_bettor_opt_in_err_user_already_opt_in() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let bettor = Keypair::new();
        program_test.add_account(
            bettor.pubkey(),
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
        let book_pda_state = Book {
            total_oracle_stake: 0,
            aggregated_oracle_outcome: None,
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
            positions: BTreeMap::from([(
                bettor.pubkey(),
                Position {
                    active_bets_count: 0,
                    bets_count: 0,
                    payout_for: 0,
                    payout_against: 0,
                    wager: 0,
                    dealt_wager: 0,
                    dispute_stake: 0,
                },
            )]),
        };
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

        let (bettor_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), bettor.pubkey().as_ref()], &program_id);
        let bettor_pda_state = UserAccount {
            authority: bettor.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::new(),
            books_bet_on: VecDeque::from([book_pda]),
        };
        let mut bettor_pda_data: Vec<u8> = Vec::new();
        bettor_pda_state.try_serialize(&mut bettor_pda_data).unwrap();
        program_test.add_account(
            bettor_pda,
            Account {
                lamports: Rent::default().minimum_balance(bettor_pda_state.current_space()),
                data: bettor_pda_data,
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
            .signer(&bettor)
            .accounts(crate::accounts::BookBettorOptInAccounts {
                bettor: bettor.pubkey(),
                bettor_user_account: bettor_pda,
                book_pda,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorOptInt)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &bettor],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be added to the bettor user account
        let bettor_user_account = banks_client.get_account(bettor_pda).await.unwrap().unwrap();
        let bettor_user_account_state = UserAccount::try_deserialize(&mut bettor_user_account.data.as_slice()).unwrap();
        assert!(bettor_user_account_state.books_bet_on.contains(&book_pda));
        // a position should be created in the book pda account
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.positions.contains_key(&bettor.pubkey()));
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_bettor_opt_in_err_opt_in_after_conclusion() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let bettor = Keypair::new();
        program_test.add_account(
            bettor.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let (bettor_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), bettor.pubkey().as_ref()], &program_id);
        let bettor_pda_state = UserAccount {
            authority: bettor.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::new(),
            books_bet_on: VecDeque::new(),
        };
        let mut bettor_pda_data: Vec<u8> = Vec::new();
        bettor_pda_state.try_serialize(&mut bettor_pda_data).unwrap();
        program_test.add_account(
            bettor_pda,
            Account {
                lamports: Rent::default().minimum_balance(bettor_pda_state.current_space()),
                data: bettor_pda_data,
                owner: program_id,
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
        let book_pda_state = Book {
            total_oracle_stake: 0,
            aggregated_oracle_outcome: None,
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
        };
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
            .signer(&bettor)
            .accounts(crate::accounts::BookBettorOptInAccounts {
                bettor: bettor.pubkey(),
                bettor_user_account: bettor_pda,
                book_pda,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorOptInt)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &bettor],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be added to the bettor user account
        let bettor_user_account = banks_client.get_account(bettor_pda).await.unwrap().unwrap();
        let bettor_user_account_state = UserAccount::try_deserialize(&mut bettor_user_account.data.as_slice()).unwrap();
        assert!(bettor_user_account_state.books_bet_on.contains(&book_pda));
        // a position should be created in the book pda account
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.positions.contains_key(&bettor.pubkey()));
    }
}
