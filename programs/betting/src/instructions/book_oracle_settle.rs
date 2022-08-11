use anchor_lang::prelude::*;
use anchor_spl::{
    mint::USDC,
    token::{Token, TokenAccount},
};

use crate::{
    constants::{BETTOR_DISPUTE_WINDOW, BETTOR_PAYOUT_RATE, ORALCES_REWARD_SHARE, RENT_PER_ORACLE},
    error::BettingError,
    state::{Book, UserAccount},
};

#[derive(Accounts)]
pub struct BookOracleSettleAccounts<'info> {
    /// CHECK: will be checked by user account seeds and in the instruction
    #[account(mut)]
    pub oracle: UncheckedAccount<'info>,
    #[account(mut,seeds=[b"UserAccount".as_ref(),oracle.key().as_ref()],bump)]
    pub oracle_user_account: Account<'info, UserAccount>,
    #[account(mut,token::mint=USDC,token::authority=oracle)]
    pub oracle_token_account: Account<'info, TokenAccount>,
    #[account(mut,seeds=[b"Book".as_ref(),&book_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
    #[account(mut,associated_token::mint=USDC,associated_token::authority=book_pda)]
    pub book_ata: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn book_oracle_settle(ctx: Context<BookOracleSettleAccounts>) -> Result<()> {
    // must be concluded
    require!(ctx.accounts.book_pda.concluded_at.is_some(), BettingError::NotInWindow);
    // must have passed the dispute window
    let now = Clock::get()?.unix_timestamp;
    require!(
        ctx.accounts.book_pda.concluded_at.unwrap() + BETTOR_DISPUTE_WINDOW < now,
        BettingError::NotInWindow
    );
    // bettors should all be settled
    require!(
        ctx.accounts.book_pda.bets_for.is_empty()
            && ctx.accounts.book_pda.bets_against.is_empty()
            && ctx.accounts.book_pda.positions.is_empty(),
        BettingError::BookNotSettled
    );
    // update user account
    if let Ok(index) = ctx
        .accounts
        .oracle_user_account
        .books_oracled
        .binary_search(&ctx.accounts.book_pda.key())
    {
        ctx.accounts.oracle_user_account.books_oracled.remove(index);
    };
    // update book pda
    match ctx.accounts.book_pda.oracles.remove(ctx.accounts.oracle.key) {
        Some(o) => {
            let final_outcome = if ctx.accounts.book_pda.total_dispute_stake > 0 {
                ctx.accounts.book_pda.dispute_resolution_result
            } else {
                ctx.accounts.book_pda.aggregated_oracle_outcome
            };
            if o.outcome == final_outcome {
                // oracle gave the correct result, pay
                let mut usdc_to_transfer = 0;
                usdc_to_transfer += o.stake; // return stake
                let total_profit = ctx.accounts.book_pda.dealt_wager * (10000 - BETTOR_PAYOUT_RATE) / 10000;
                let total_oralce_reward = total_profit * ORALCES_REWARD_SHARE / 10000;
                usdc_to_transfer += total_oralce_reward * o.stake / ctx.accounts.book_pda.total_oracle_stake;
                // transfer usdc
                let usdc_transfer_cpi_context = CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    anchor_spl::token::Transfer {
                        from: ctx.accounts.book_ata.to_account_info(),
                        to: ctx.accounts.oracle_token_account.to_account_info(),
                        authority: ctx.accounts.book_pda.to_account_info(),
                    },
                );
                let bet_type_vec = ctx.accounts.book_pda.bet_type.try_to_vec().unwrap();
                let book_pda_signer_seeds = &[
                    b"Book".as_ref(),
                    &ctx.accounts.book_pda.game_id.to_le_bytes(),
                    bet_type_vec.as_slice(),
                    &[*ctx.bumps.get("book_pda").unwrap()],
                ];
                anchor_spl::token::transfer(
                    usdc_transfer_cpi_context.with_signer(&[book_pda_signer_seeds]),
                    usdc_to_transfer,
                )?;
            } else {
                // oracle gave the wrong result, no pay
            }
            // realloc
            let book_pda_account_info = ctx.accounts.book_pda.to_account_info();
            book_pda_account_info.realloc(ctx.accounts.book_pda.current_space(), false)?;
            // return lamports
            let oracle_account_info = ctx.accounts.oracle.to_account_info();
            **oracle_account_info.lamports.borrow_mut() =
                oracle_account_info.lamports().checked_add(RENT_PER_ORACLE).unwrap();
            **book_pda_account_info.lamports.borrow_mut() =
                book_pda_account_info.lamports().checked_sub(RENT_PER_ORACLE).unwrap();
        }
        None => {
            return err!(BettingError::NotFound);
        }
    };
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
    use anchor_spl::mint::USDC;
    use solana_program_test::{tokio, ProgramTest};
    use solana_sdk::{
        account::Account, native_token::LAMPORTS_PER_SOL, program_pack::Pack, pubkey::Pubkey, rent::Rent,
        signature::Keypair, signer::Signer, system_program, transaction::Transaction,
    };

    use crate::{
        constants::{BETTOR_DISPUTE_WINDOW, RENT_PER_ORACLE},
        state::{Bet, BetOutcome, BetType, Book, Oracle, Position, UserAccount},
    };

    #[tokio::test]
    async fn test_book_oracle_settle_success_with_oracle_right_result() {
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

        let oracle_ata = anchor_spl::associated_token::get_associated_token_address(&oracle.pubkey(), &USDC);
        let oracle_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: oracle.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut oracle_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(oracle_ata_state, &mut oracle_ata_data).unwrap();
        program_test.add_account(
            oracle_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(oracle_ata_data),
                owner: anchor_spl::token::ID,
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
            total_oracle_stake: 1000000 * 100,
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 1000000 * 500,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 500,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::from([(
                oracle.pubkey(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::For),
                },
            )]),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::For),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
                data: book_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 1000000 * 1000,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut book_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(book_ata_state, &mut book_ata_data).unwrap();
        program_test.add_account(
            book_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(book_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

        let (oracle_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), oracle.pubkey().as_ref()], &program_id);
        let oracle_pda_state = UserAccount {
            authority: oracle.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::from([book_pda]),
            books_bet_on: VecDeque::new(),
        };
        let mut oracle_pda_data: Vec<u8> = Vec::new();
        oracle_pda_state.try_serialize(&mut oracle_pda_data).unwrap();
        program_test.add_account(
            oracle_pda,
            Account {
                lamports: Rent::default().minimum_balance(oracle_pda_state.current_space()),
                data: oracle_pda_data,
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
            .accounts(crate::accounts::BookOracleSettleAccounts {
                oracle: oracle.pubkey(),
                oracle_user_account: oracle_pda,
                oracle_token_account: oracle_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookOracleSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // rent should be returned to the oracle system account
        let oracle_system_account = banks_client.get_account(oracle.pubkey()).await.unwrap().unwrap();
        assert_eq!(oracle_system_account.lamports, LAMPORTS_PER_SOL + RENT_PER_ORACLE);
        // the book pda should be removed from the oracle user account
        let oracle_user_account = banks_client.get_account(oracle_pda).await.unwrap().unwrap();
        let oracle_user_account_state = UserAccount::try_deserialize(&mut oracle_user_account.data.as_slice()).unwrap();
        assert!(!oracle_user_account_state.books_oracled.contains(&book_pda));
        // reward and oracle stake should be transferred to the oracle token account
        let user_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(oracle_ata).await.unwrap();
        assert_eq!(user_token_account_state.amount, 1000000 * 100 + 1000000 * 100 + 3000000);
        // the oracle should be removed from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.oracles.get(&oracle.pubkey()).is_none());
        // reward and oracle stake should be transferred from the the book ata
        let book_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(
            book_token_account_state.amount,
            1000000 * 1000 - 1000000 * 100 - 3000000
        );
    }

    #[tokio::test]
    async fn test_book_oracle_settle_success_with_oracle_wrong_result() {
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

        let oracle_ata = anchor_spl::associated_token::get_associated_token_address(&oracle.pubkey(), &USDC);
        let oracle_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: oracle.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut oracle_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(oracle_ata_state, &mut oracle_ata_data).unwrap();
        program_test.add_account(
            oracle_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(oracle_ata_data),
                owner: anchor_spl::token::ID,
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
            total_oracle_stake: 1000000 * 100,
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 1000000 * 500,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 500,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::from([(
                oracle.pubkey(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::For),
                },
            )]),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::Against),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
                data: book_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 1000000 * 1000,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut book_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(book_ata_state, &mut book_ata_data).unwrap();
        program_test.add_account(
            book_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(book_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

        let (oracle_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), oracle.pubkey().as_ref()], &program_id);
        let oracle_pda_state = UserAccount {
            authority: oracle.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::from([book_pda]),
            books_bet_on: VecDeque::new(),
        };
        let mut oracle_pda_data: Vec<u8> = Vec::new();
        oracle_pda_state.try_serialize(&mut oracle_pda_data).unwrap();
        program_test.add_account(
            oracle_pda,
            Account {
                lamports: Rent::default().minimum_balance(oracle_pda_state.current_space()),
                data: oracle_pda_data,
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
            .accounts(crate::accounts::BookOracleSettleAccounts {
                oracle: oracle.pubkey(),
                oracle_user_account: oracle_pda,
                oracle_token_account: oracle_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookOracleSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // rent should be returned to the oracle system account
        let oracle_system_account = banks_client.get_account(oracle.pubkey()).await.unwrap().unwrap();
        assert_eq!(oracle_system_account.lamports, LAMPORTS_PER_SOL + RENT_PER_ORACLE);
        // the book pda should be removed from the oracle user account
        let oracle_user_account = banks_client.get_account(oracle_pda).await.unwrap().unwrap();
        let oracle_user_account_state = UserAccount::try_deserialize(&mut oracle_user_account.data.as_slice()).unwrap();
        assert!(!oracle_user_account_state.books_oracled.contains(&book_pda));
        // reward and oracle stake should not be transferred to the oracle token account
        let user_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(oracle_ata).await.unwrap();
        assert_eq!(user_token_account_state.amount, 1000000 * 100);
        // the oracle should be removed from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.oracles.get(&oracle.pubkey()).is_none());
        // reward and oracle stake should not be transferred from the the book ata
        let book_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_token_account_state.amount, 1000000 * 1000);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_oracle_settle_err_not_concluded() {
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

        let oracle_ata = anchor_spl::associated_token::get_associated_token_address(&oracle.pubkey(), &USDC);
        let oracle_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: oracle.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut oracle_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(oracle_ata_state, &mut oracle_ata_data).unwrap();
        program_test.add_account(
            oracle_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(oracle_ata_data),
                owner: anchor_spl::token::ID,
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
            total_oracle_stake: 1000000 * 100,
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 1000000 * 500,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 500,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: None,
            oracles: BTreeMap::from([(
                oracle.pubkey(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::For),
                },
            )]),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::For),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
                data: book_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 1000000 * 1000,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut book_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(book_ata_state, &mut book_ata_data).unwrap();
        program_test.add_account(
            book_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(book_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

        let (oracle_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), oracle.pubkey().as_ref()], &program_id);
        let oracle_pda_state = UserAccount {
            authority: oracle.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::from([book_pda]),
            books_bet_on: VecDeque::new(),
        };
        let mut oracle_pda_data: Vec<u8> = Vec::new();
        oracle_pda_state.try_serialize(&mut oracle_pda_data).unwrap();
        program_test.add_account(
            oracle_pda,
            Account {
                lamports: Rent::default().minimum_balance(oracle_pda_state.current_space()),
                data: oracle_pda_data,
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
            .accounts(crate::accounts::BookOracleSettleAccounts {
                oracle: oracle.pubkey(),
                oracle_user_account: oracle_pda,
                oracle_token_account: oracle_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookOracleSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // rent should be returned to the oracle system account
        let oracle_system_account = banks_client.get_account(oracle.pubkey()).await.unwrap().unwrap();
        assert_eq!(oracle_system_account.lamports, LAMPORTS_PER_SOL + RENT_PER_ORACLE);
        // the book pda should be removed from the oracle user account
        let oracle_user_account = banks_client.get_account(oracle_pda).await.unwrap().unwrap();
        let oracle_user_account_state = UserAccount::try_deserialize(&mut oracle_user_account.data.as_slice()).unwrap();
        assert!(!oracle_user_account_state.books_oracled.contains(&book_pda));
        // reward and oracle stake should be transferred to the oracle token account
        let user_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(oracle_ata).await.unwrap();
        assert_eq!(user_token_account_state.amount, 1000000 * 100 + 1000000 * 100 + 3000000);
        // the oracle should be removed from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.oracles.get(&oracle.pubkey()).is_none());
        // reward and oracle stake should be transferred from the the book ata
        let book_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(
            book_token_account_state.amount,
            1000000 * 1000 - 1000000 * 100 - 3000000
        );
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_oracle_settle_bettor_dispute_window_not_passed() {
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

        let oracle_ata = anchor_spl::associated_token::get_associated_token_address(&oracle.pubkey(), &USDC);
        let oracle_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: oracle.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut oracle_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(oracle_ata_state, &mut oracle_ata_data).unwrap();
        program_test.add_account(
            oracle_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(oracle_ata_data),
                owner: anchor_spl::token::ID,
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
            total_oracle_stake: 1000000 * 100,
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 1000000 * 500,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 500,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(chrono::Utc::now().timestamp() - BETTOR_DISPUTE_WINDOW + 60),
            oracles: BTreeMap::from([(
                oracle.pubkey(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::For),
                },
            )]),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::For),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
                data: book_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 1000000 * 1000,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut book_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(book_ata_state, &mut book_ata_data).unwrap();
        program_test.add_account(
            book_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(book_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

        let (oracle_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), oracle.pubkey().as_ref()], &program_id);
        let oracle_pda_state = UserAccount {
            authority: oracle.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::from([book_pda]),
            books_bet_on: VecDeque::new(),
        };
        let mut oracle_pda_data: Vec<u8> = Vec::new();
        oracle_pda_state.try_serialize(&mut oracle_pda_data).unwrap();
        program_test.add_account(
            oracle_pda,
            Account {
                lamports: Rent::default().minimum_balance(oracle_pda_state.current_space()),
                data: oracle_pda_data,
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
            .accounts(crate::accounts::BookOracleSettleAccounts {
                oracle: oracle.pubkey(),
                oracle_user_account: oracle_pda,
                oracle_token_account: oracle_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookOracleSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // rent should be returned to the oracle system account
        let oracle_system_account = banks_client.get_account(oracle.pubkey()).await.unwrap().unwrap();
        assert_eq!(oracle_system_account.lamports, LAMPORTS_PER_SOL + RENT_PER_ORACLE);
        // the book pda should be removed from the oracle user account
        let oracle_user_account = banks_client.get_account(oracle_pda).await.unwrap().unwrap();
        let oracle_user_account_state = UserAccount::try_deserialize(&mut oracle_user_account.data.as_slice()).unwrap();
        assert!(!oracle_user_account_state.books_oracled.contains(&book_pda));
        // reward and oracle stake should be transferred to the oracle token account
        let user_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(oracle_ata).await.unwrap();
        assert_eq!(user_token_account_state.amount, 1000000 * 100 + 1000000 * 100 + 3000000);
        // the oracle should be removed from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.oracles.get(&oracle.pubkey()).is_none());
        // reward and oracle stake should be transferred from the the book ata
        let book_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(
            book_token_account_state.amount,
            1000000 * 1000 - 1000000 * 100 - 3000000
        );
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6002)")]
    async fn test_book_oracle_settle_err_bettors_not_settled() {
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

        let oracle_ata = anchor_spl::associated_token::get_associated_token_address(&oracle.pubkey(), &USDC);
        let oracle_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: oracle.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut oracle_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(oracle_ata_state, &mut oracle_ata_data).unwrap();
        program_test.add_account(
            oracle_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(oracle_ata_data),
                owner: anchor_spl::token::ID,
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
        let unsettled_bettor = Pubkey::new_unique();
        let book_pda_state = Book {
            total_oracle_stake: 1000000 * 100,
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 1000000 * 500,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 500,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::from([(
                oracle.pubkey(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::For),
                },
            )]),
            bets_for: VecDeque::from([Bet {
                id: 0,
                bettor: unsettled_bettor,
                wager: 1000000 * 100,
            }]),
            bets_against: VecDeque::new(),
            positions: BTreeMap::from([(
                unsettled_bettor,
                Position {
                    active_bets_count: 1,
                    bets_count: 1,
                    payout_for: 0,
                    payout_against: 0,
                    wager: 1000000 * 100,
                    dealt_wager: 0,
                    dispute_stake: 0,
                },
            )]),
            aggregated_oracle_outcome: Some(BetOutcome::For),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
                data: book_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 1000000 * 1000,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut book_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(book_ata_state, &mut book_ata_data).unwrap();
        program_test.add_account(
            book_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(book_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

        let (oracle_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), oracle.pubkey().as_ref()], &program_id);
        let oracle_pda_state = UserAccount {
            authority: oracle.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::from([book_pda]),
            books_bet_on: VecDeque::new(),
        };
        let mut oracle_pda_data: Vec<u8> = Vec::new();
        oracle_pda_state.try_serialize(&mut oracle_pda_data).unwrap();
        program_test.add_account(
            oracle_pda,
            Account {
                lamports: Rent::default().minimum_balance(oracle_pda_state.current_space()),
                data: oracle_pda_data,
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
            .accounts(crate::accounts::BookOracleSettleAccounts {
                oracle: oracle.pubkey(),
                oracle_user_account: oracle_pda,
                oracle_token_account: oracle_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookOracleSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // rent should be returned to the oracle system account
        let oracle_system_account = banks_client.get_account(oracle.pubkey()).await.unwrap().unwrap();
        assert_eq!(oracle_system_account.lamports, LAMPORTS_PER_SOL + RENT_PER_ORACLE);
        // the book pda should be removed from the oracle user account
        let oracle_user_account = banks_client.get_account(oracle_pda).await.unwrap().unwrap();
        let oracle_user_account_state = UserAccount::try_deserialize(&mut oracle_user_account.data.as_slice()).unwrap();
        assert!(!oracle_user_account_state.books_oracled.contains(&book_pda));
        // reward and oracle stake should be transferred to the oracle token account
        let user_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(oracle_ata).await.unwrap();
        assert_eq!(user_token_account_state.amount, 1000000 * 100 + 1000000 * 100 + 3000000);
        // the oracle should be removed from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.oracles.get(&oracle.pubkey()).is_none());
        // reward and oracle stake should be transferred from the the book ata
        let book_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(
            book_token_account_state.amount,
            1000000 * 1000 - 1000000 * 100 - 3000000
        );
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6007)")]
    async fn test_book_oracle_settle_err_wrong_oracle() {
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

        let oracle_ata = anchor_spl::associated_token::get_associated_token_address(&oracle.pubkey(), &USDC);
        let oracle_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: oracle.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut oracle_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(oracle_ata_state, &mut oracle_ata_data).unwrap();
        program_test.add_account(
            oracle_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(oracle_ata_data),
                owner: anchor_spl::token::ID,
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
            total_oracle_stake: 1000000 * 100,
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 1000000 * 500,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 500,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::from([(
                Pubkey::new_unique(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::For),
                },
            )]),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::For),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        book_pda_data.resize(book_pda_state.current_space(), 0);
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
                data: book_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 1000000 * 1000,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut book_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(book_ata_state, &mut book_ata_data).unwrap();
        program_test.add_account(
            book_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(book_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

        let (oracle_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), oracle.pubkey().as_ref()], &program_id);
        let oracle_pda_state = UserAccount {
            authority: oracle.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::from([book_pda]),
            books_bet_on: VecDeque::new(),
        };
        let mut oracle_pda_data: Vec<u8> = Vec::new();
        oracle_pda_state.try_serialize(&mut oracle_pda_data).unwrap();
        program_test.add_account(
            oracle_pda,
            Account {
                lamports: Rent::default().minimum_balance(oracle_pda_state.current_space()),
                data: oracle_pda_data,
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
            .accounts(crate::accounts::BookOracleSettleAccounts {
                oracle: oracle.pubkey(),
                oracle_user_account: oracle_pda,
                oracle_token_account: oracle_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookOracleSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // rent should be returned to the oracle system account
        let oracle_system_account = banks_client.get_account(oracle.pubkey()).await.unwrap().unwrap();
        assert_eq!(oracle_system_account.lamports, LAMPORTS_PER_SOL + RENT_PER_ORACLE);
        // the book pda should be removed from the oracle user account
        let oracle_user_account = banks_client.get_account(oracle_pda).await.unwrap().unwrap();
        let oracle_user_account_state = UserAccount::try_deserialize(&mut oracle_user_account.data.as_slice()).unwrap();
        assert!(!oracle_user_account_state.books_oracled.contains(&book_pda));
        // reward and oracle stake should be transferred to the oracle token account
        let user_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(oracle_ata).await.unwrap();
        assert_eq!(user_token_account_state.amount, 1000000 * 100 + 1000000 * 100 + 3000000);
        // the oracle should be removed from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.oracles.get(&oracle.pubkey()).is_none());
        // reward and oracle stake should be transferred from the the book ata
        let book_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(
            book_token_account_state.amount,
            1000000 * 1000 - 1000000 * 100 - 3000000
        );
    }
}
