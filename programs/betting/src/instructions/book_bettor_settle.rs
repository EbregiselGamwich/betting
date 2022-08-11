use anchor_lang::prelude::*;
use anchor_spl::{
    mint::USDC,
    token::{Token, TokenAccount},
};

use crate::{
    constants::{BETTOR_DISPUTE_WINDOW, BETTOR_PAYOUT_RATE, RENT_PER_BET, RENT_PER_POSITION},
    error::BettingError,
    state::{BetOutcome, Book, UserAccount},
};

#[derive(Accounts)]
pub struct BookBettorSettleAccounts<'info> {
    #[account(mut)]
    pub bettor: UncheckedAccount<'info>,
    #[account(mut,seeds=[b"UserAccount".as_ref(),bettor.key().as_ref()],bump)]
    pub bettor_user_account: Account<'info, UserAccount>,
    #[account(mut,token::mint=USDC,token::authority=bettor)]
    pub bettor_token_account: Account<'info, TokenAccount>,
    #[account(mut,seeds=[b"Book".as_ref(),&book_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
    #[account(mut,associated_token::mint=USDC,associated_token::authority=book_pda)]
    pub book_ata: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn book_bettor_settle(ctx: Context<BookBettorSettleAccounts>) -> Result<()> {
    // must be concluded
    require!(ctx.accounts.book_pda.concluded_at.is_some(), BettingError::NotInWindow);
    // must have passed the dispute window
    let now = Clock::get()?.unix_timestamp;
    require!(
        ctx.accounts.book_pda.concluded_at.unwrap() + BETTOR_DISPUTE_WINDOW < now,
        BettingError::NotInWindow
    );
    // must have an outcome
    let oracle_outcome = ctx.accounts.book_pda.aggregated_outcome();
    let dispute_resolution_outcome = ctx.accounts.book_pda.dispute_resolution_result;
    let final_outcome = if ctx.accounts.book_pda.total_dispute_stake > 0 {
        dispute_resolution_outcome
    } else {
        oracle_outcome
    };
    require!(final_outcome.is_some(), BettingError::NoResultYet);
    // update bettor user account
    if let Ok(index) = ctx
        .accounts
        .bettor_user_account
        .books_bet_on
        .binary_search(&ctx.accounts.book_pda.key())
    {
        ctx.accounts.bettor_user_account.books_bet_on.remove(index);
    }
    // update book pda
    // remove all active bets
    ctx.accounts
        .book_pda
        .bets_for
        .retain(|b| b.bettor != ctx.accounts.bettor.key());
    ctx.accounts
        .book_pda
        .bets_against
        .retain(|b| b.bettor != ctx.accounts.bettor.key());
    // remove position
    match ctx.accounts.book_pda.positions.remove(ctx.accounts.bettor.key) {
        Some(p) => {
            // calculate usdc to transfer
            let mut usdc_to_transfer = 0;
            match final_outcome.unwrap() {
                BetOutcome::For => {
                    usdc_to_transfer += p.payout_for * BETTOR_PAYOUT_RATE / 10000;
                    usdc_to_transfer += p.wager - p.dealt_wager;
                }
                BetOutcome::Cancel => {
                    usdc_to_transfer += p.wager;
                }
                BetOutcome::Against => {
                    usdc_to_transfer += p.payout_against * BETTOR_PAYOUT_RATE / 10000;
                    usdc_to_transfer += p.wager - p.dealt_wager;
                }
            }
            // return dispute stake if the oracles are wrong
            if final_outcome != oracle_outcome {
                usdc_to_transfer += p.dispute_stake;
            }
            // transfer usdc
            let usdc_transfer_cpi_context = CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token::Transfer {
                    from: ctx.accounts.book_ata.to_account_info(),
                    to: ctx.accounts.bettor_token_account.to_account_info(),
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
            // realloc
            let book_pda_account_info = ctx.accounts.book_pda.to_account_info();
            book_pda_account_info.realloc(ctx.accounts.book_pda.current_space(), false)?;
            // return lamports
            let lamports_to_return = RENT_PER_BET * (p.bets_count as u64) + RENT_PER_POSITION;
            let bettor_account_info = ctx.accounts.bettor.to_account_info();
            **bettor_account_info.lamports.borrow_mut() =
                bettor_account_info.lamports().checked_add(lamports_to_return).unwrap();
            **book_pda_account_info.lamports.borrow_mut() = book_pda_account_info
                .lamports()
                .checked_sub(lamports_to_return)
                .unwrap();
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
        constants::{BETTOR_DISPUTE_WINDOW, BETTOR_PAYOUT_RATE, RENT_PER_BET, RENT_PER_POSITION},
        state::{Bet, BetOutcome, BetType, Book, Oracle, Position, UserAccount},
    };

    #[tokio::test]
    async fn test_book_bettor_settle_success() {
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

        let bettor_ata = anchor_spl::associated_token::get_associated_token_address(&bettor.pubkey(), &USDC);
        let bettor_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: bettor.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut bettor_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(bettor_ata_state, &mut bettor_ata_data).unwrap();
        program_test.add_account(
            bettor_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(bettor_ata_data),
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
            total_oracle_stake: 0,
            aggregated_oracle_outcome: Some(BetOutcome::Against),
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 3,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 1000000 * 30,
            dispute_resolution_result: Some(BetOutcome::For),
            concluded_at: Some(0),
            oracles: BTreeMap::from([(
                Pubkey::new_unique(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::Against),
                },
            )]),
            bets_for: VecDeque::from([Bet {
                id: u64::from_le_bytes([0x02, 0x00, 0x00, 0x00, 0xb0, 0x04, 0x00, 0x00]),
                bettor: bettor.pubkey(),
                wager: 1000000 * 20,
            }]),
            bets_against: VecDeque::new(),
            positions: BTreeMap::from([(
                bettor.pubkey(),
                Position {
                    active_bets_count: 1,
                    bets_count: 3,
                    payout_for: 1000000 * 100,
                    payout_against: 1000000 * 200,
                    wager: 1000000 * 400,
                    dealt_wager: 1000000 * 300,
                    dispute_stake: 1000000 * 30,
                },
            )]),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
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
            books_bet_on: VecDeque::from(vec![book_pda]),
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

        let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

        let rb = RequestBuilder::from(
            program_id,
            "",
            Rc::new(Keypair::new()),
            None,
            anchor_client::RequestNamespace::Global,
        );
        let instructions = rb
            .accounts(crate::accounts::BookBettorSettleAccounts {
                bettor: bettor.pubkey(),
                bettor_user_account: bettor_pda,
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();
        // rent for the position and bets should be return to the bettor
        let bettor_account = banks_client.get_account(bettor.pubkey()).await.unwrap().unwrap();
        assert_eq!(
            bettor_account.lamports,
            LAMPORTS_PER_SOL + RENT_PER_POSITION + 3 * RENT_PER_BET
        );
        // the book pda should be removed from the user account
        let bettor_user_account = banks_client.get_account(bettor_pda).await.unwrap().unwrap();
        let bettor_user_account_state = UserAccount::try_deserialize(&mut bettor_user_account.data.as_slice()).unwrap();
        assert!(!bettor_user_account_state.books_bet_on.contains(&book_pda));
        // usdc should be transferred to the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(
            bettor_token_account_state.amount,
            1000000 * 230 + 1000000 * 100 * BETTOR_PAYOUT_RATE / 10000
        );
        // lamports should be taken out from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        assert_eq!(
            book_account.lamports,
            LAMPORTS_PER_SOL - RENT_PER_POSITION - 3 * RENT_PER_BET
        );
        // the bets and the position of the bettor should be removed from the book pda
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.bets_for.is_empty());
        assert!(book_state.bets_against.is_empty());
        assert!(book_state.positions.get(&bettor.pubkey()).is_none());
        // usdc should be transferred from the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(
            book_ata_account_state.amount,
            1000000 * 1000 - (1000000 * 130 + 1000000 * 100 * BETTOR_PAYOUT_RATE / 10000)
        );
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_bettor_settle_err_not_concluded() {
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

        let bettor_ata = anchor_spl::associated_token::get_associated_token_address(&bettor.pubkey(), &USDC);
        let bettor_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: bettor.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut bettor_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(bettor_ata_state, &mut bettor_ata_data).unwrap();
        program_test.add_account(
            bettor_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(bettor_ata_data),
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
            total_oracle_stake: 0,
            aggregated_oracle_outcome: Some(BetOutcome::Against),
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 3,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 1000000 * 30,
            dispute_resolution_result: Some(BetOutcome::For),
            concluded_at: None,
            oracles: BTreeMap::from([(
                Pubkey::new_unique(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::Against),
                },
            )]),
            bets_for: VecDeque::from([Bet {
                id: u64::from_le_bytes([0x02, 0x00, 0x00, 0x00, 0xb0, 0x04, 0x00, 0x00]),
                bettor: bettor.pubkey(),
                wager: 1000000 * 20,
            }]),
            bets_against: VecDeque::new(),
            positions: BTreeMap::from([(
                bettor.pubkey(),
                Position {
                    active_bets_count: 1,
                    bets_count: 3,
                    payout_for: 1000000 * 100,
                    payout_against: 1000000 * 200,
                    wager: 1000000 * 400,
                    dealt_wager: 1000000 * 300,
                    dispute_stake: 1000000 * 30,
                },
            )]),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
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
            books_bet_on: VecDeque::from(vec![book_pda]),
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

        let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

        let rb = RequestBuilder::from(
            program_id,
            "",
            Rc::new(Keypair::new()),
            None,
            anchor_client::RequestNamespace::Global,
        );
        let instructions = rb
            .accounts(crate::accounts::BookBettorSettleAccounts {
                bettor: bettor.pubkey(),
                bettor_user_account: bettor_pda,
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();
        // rent for the position and bets should be return to the bettor
        let bettor_account = banks_client.get_account(bettor.pubkey()).await.unwrap().unwrap();
        assert_eq!(
            bettor_account.lamports,
            LAMPORTS_PER_SOL + RENT_PER_POSITION + 3 * RENT_PER_BET
        );
        // the book pda should be removed from the user account
        let bettor_user_account = banks_client.get_account(bettor_pda).await.unwrap().unwrap();
        let bettor_user_account_state = UserAccount::try_deserialize(&mut bettor_user_account.data.as_slice()).unwrap();
        assert!(!bettor_user_account_state.books_bet_on.contains(&book_pda));
        // usdc should be transferred to the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 330);
        // lamports should be taken out from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        assert_eq!(
            book_account.lamports,
            LAMPORTS_PER_SOL - RENT_PER_POSITION - 3 * RENT_PER_BET
        );
        // the bets and the position of the bettor should be removed from the book pda
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.bets_for.is_empty());
        assert!(book_state.bets_against.is_empty());
        assert!(book_state.positions.get(&bettor.pubkey()).is_none());
        // usdc should be transferred from the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 1000000 * (1000 - 230));
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_bettor_settle_err_still_in_dispute_window() {
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

        let bettor_ata = anchor_spl::associated_token::get_associated_token_address(&bettor.pubkey(), &USDC);
        let bettor_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: bettor.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut bettor_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(bettor_ata_state, &mut bettor_ata_data).unwrap();
        program_test.add_account(
            bettor_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(bettor_ata_data),
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
            total_oracle_stake: 0,
            aggregated_oracle_outcome: Some(BetOutcome::Against),
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 3,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 1000000 * 30,
            dispute_resolution_result: Some(BetOutcome::For),
            concluded_at: Some(chrono::Utc::now().timestamp() - BETTOR_DISPUTE_WINDOW + 60),
            oracles: BTreeMap::from([(
                Pubkey::new_unique(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::Against),
                },
            )]),
            bets_for: VecDeque::from([Bet {
                id: u64::from_le_bytes([0x02, 0x00, 0x00, 0x00, 0xb0, 0x04, 0x00, 0x00]),
                bettor: bettor.pubkey(),
                wager: 1000000 * 20,
            }]),
            bets_against: VecDeque::new(),
            positions: BTreeMap::from([(
                bettor.pubkey(),
                Position {
                    active_bets_count: 1,
                    bets_count: 3,
                    payout_for: 1000000 * 100,
                    payout_against: 1000000 * 200,
                    wager: 1000000 * 400,
                    dealt_wager: 1000000 * 300,
                    dispute_stake: 1000000 * 30,
                },
            )]),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
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
            books_bet_on: VecDeque::from(vec![book_pda]),
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

        let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

        let rb = RequestBuilder::from(
            program_id,
            "",
            Rc::new(Keypair::new()),
            None,
            anchor_client::RequestNamespace::Global,
        );
        let instructions = rb
            .accounts(crate::accounts::BookBettorSettleAccounts {
                bettor: bettor.pubkey(),
                bettor_user_account: bettor_pda,
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();
        // rent for the position and bets should be return to the bettor
        let bettor_account = banks_client.get_account(bettor.pubkey()).await.unwrap().unwrap();
        assert_eq!(
            bettor_account.lamports,
            LAMPORTS_PER_SOL + RENT_PER_POSITION + 3 * RENT_PER_BET
        );
        // the book pda should be removed from the user account
        let bettor_user_account = banks_client.get_account(bettor_pda).await.unwrap().unwrap();
        let bettor_user_account_state = UserAccount::try_deserialize(&mut bettor_user_account.data.as_slice()).unwrap();
        assert!(!bettor_user_account_state.books_bet_on.contains(&book_pda));
        // usdc should be transferred to the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 330);
        // lamports should be taken out from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        assert_eq!(
            book_account.lamports,
            LAMPORTS_PER_SOL - RENT_PER_POSITION - 3 * RENT_PER_BET
        );
        // the bets and the position of the bettor should be removed from the book pda
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.bets_for.is_empty());
        assert!(book_state.bets_against.is_empty());
        assert!(book_state.positions.get(&bettor.pubkey()).is_none());
        // usdc should be transferred from the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 1000000 * (1000 - 230));
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6008)")]
    async fn test_book_bettor_settle_err_no_result_from_oracle_or_dispute_resolution() {
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

        let bettor_ata = anchor_spl::associated_token::get_associated_token_address(&bettor.pubkey(), &USDC);
        let bettor_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: bettor.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut bettor_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(bettor_ata_state, &mut bettor_ata_data).unwrap();
        program_test.add_account(
            bettor_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(bettor_ata_data),
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
            total_oracle_stake: 0,
            aggregated_oracle_outcome: Some(BetOutcome::Against),
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 3,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 1000000 * 30,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::from([(
                Pubkey::new_unique(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::Against),
                },
            )]),
            bets_for: VecDeque::from([Bet {
                id: u64::from_le_bytes([0x02, 0x00, 0x00, 0x00, 0xb0, 0x04, 0x00, 0x00]),
                bettor: bettor.pubkey(),
                wager: 1000000 * 20,
            }]),
            bets_against: VecDeque::new(),
            positions: BTreeMap::from([(
                bettor.pubkey(),
                Position {
                    active_bets_count: 1,
                    bets_count: 3,
                    payout_for: 1000000 * 100,
                    payout_against: 1000000 * 200,
                    wager: 1000000 * 400,
                    dealt_wager: 1000000 * 300,
                    dispute_stake: 1000000 * 30,
                },
            )]),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
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
            books_bet_on: VecDeque::from(vec![book_pda]),
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

        let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

        let rb = RequestBuilder::from(
            program_id,
            "",
            Rc::new(Keypair::new()),
            None,
            anchor_client::RequestNamespace::Global,
        );
        let instructions = rb
            .accounts(crate::accounts::BookBettorSettleAccounts {
                bettor: bettor.pubkey(),
                bettor_user_account: bettor_pda,
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();
        // rent for the position and bets should be return to the bettor
        let bettor_account = banks_client.get_account(bettor.pubkey()).await.unwrap().unwrap();
        assert_eq!(
            bettor_account.lamports,
            LAMPORTS_PER_SOL + RENT_PER_POSITION + 3 * RENT_PER_BET
        );
        // the book pda should be removed from the user account
        let bettor_user_account = banks_client.get_account(bettor_pda).await.unwrap().unwrap();
        let bettor_user_account_state = UserAccount::try_deserialize(&mut bettor_user_account.data.as_slice()).unwrap();
        assert!(!bettor_user_account_state.books_bet_on.contains(&book_pda));
        // usdc should be transferred to the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 330);
        // lamports should be taken out from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        assert_eq!(
            book_account.lamports,
            LAMPORTS_PER_SOL - RENT_PER_POSITION - 3 * RENT_PER_BET
        );
        // the bets and the position of the bettor should be removed from the book pda
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.bets_for.is_empty());
        assert!(book_state.bets_against.is_empty());
        assert!(book_state.positions.get(&bettor.pubkey()).is_none());
        // usdc should be transferred from the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 1000000 * (1000 - 230));
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6007)")]
    async fn test_book_bettor_settle_err_wrong_bettor() {
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

        let bettor_ata = anchor_spl::associated_token::get_associated_token_address(&bettor.pubkey(), &USDC);
        let bettor_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: bettor.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut bettor_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(bettor_ata_state, &mut bettor_ata_data).unwrap();
        program_test.add_account(
            bettor_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(bettor_ata_data),
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
            total_oracle_stake: 0,
            aggregated_oracle_outcome: Some(BetOutcome::Against),
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 3,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 0,
            bet_type,
            total_dispute_stake: 1000000 * 30,
            dispute_resolution_result: Some(BetOutcome::For),
            concluded_at: Some(0),
            oracles: BTreeMap::from([(
                Pubkey::new_unique(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: Some(BetOutcome::Against),
                },
            )]),
            bets_for: VecDeque::from([Bet {
                id: u64::from_le_bytes([0x02, 0x00, 0x00, 0x00, 0xb0, 0x04, 0x00, 0x00]),
                bettor: bettor.pubkey(),
                wager: 1000000 * 20,
            }]),
            bets_against: VecDeque::new(),
            positions: BTreeMap::from([(
                Pubkey::new_unique(),
                Position {
                    active_bets_count: 1,
                    bets_count: 3,
                    payout_for: 1000000 * 100,
                    payout_against: 1000000 * 200,
                    wager: 1000000 * 400,
                    dealt_wager: 1000000 * 300,
                    dispute_stake: 1000000 * 30,
                },
            )]),
        };
        let mut book_pda_data: Vec<u8> = Vec::new();
        book_pda_state.try_serialize(&mut book_pda_data).unwrap();
        program_test.add_account(
            book_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
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
            books_bet_on: VecDeque::from(vec![book_pda]),
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

        let (mut banks_client, payer, recent_blockhash) = program_test.start().await;

        let rb = RequestBuilder::from(
            program_id,
            "",
            Rc::new(Keypair::new()),
            None,
            anchor_client::RequestNamespace::Global,
        );
        let instructions = rb
            .accounts(crate::accounts::BookBettorSettleAccounts {
                bettor: bettor.pubkey(),
                bettor_user_account: bettor_pda,
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();
        // rent for the position and bets should be return to the bettor
        let bettor_account = banks_client.get_account(bettor.pubkey()).await.unwrap().unwrap();
        assert_eq!(
            bettor_account.lamports,
            LAMPORTS_PER_SOL + RENT_PER_POSITION + 3 * RENT_PER_BET
        );
        // the book pda should be removed from the user account
        let bettor_user_account = banks_client.get_account(bettor_pda).await.unwrap().unwrap();
        let bettor_user_account_state = UserAccount::try_deserialize(&mut bettor_user_account.data.as_slice()).unwrap();
        assert!(!bettor_user_account_state.books_bet_on.contains(&book_pda));
        // usdc should be transferred to the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 330);
        // lamports should be taken out from the book pda
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        assert_eq!(
            book_account.lamports,
            LAMPORTS_PER_SOL - RENT_PER_POSITION - 3 * RENT_PER_BET
        );
        // the bets and the position of the bettor should be removed from the book pda
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.bets_for.is_empty());
        assert!(book_state.bets_against.is_empty());
        assert!(book_state.positions.get(&bettor.pubkey()).is_none());
        // usdc should be transferred from the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 1000000 * (1000 - 230));
    }
}
