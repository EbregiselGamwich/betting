use anchor_lang::{prelude::*, system_program};
use anchor_spl::{
    mint::USDC,
    token::{Token, TokenAccount},
};

use crate::{
    constants::{MIN_BET_AMOUNT, RENT_PER_BET},
    error::BettingError,
    state::{BetDirection, Book},
};

#[derive(Accounts)]
pub struct BookBettorPlaceBetAccounts<'info> {
    #[account(mut)]
    pub bettor: Signer<'info>,
    #[account(mut,token::mint=USDC,token::authority=bettor)]
    pub bettor_token_account: Account<'info, TokenAccount>,
    #[account(mut,seeds=[b"Book".as_ref(),&book_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
    #[account(mut,associated_token::mint=USDC,associated_token::authority=book_pda)]
    pub book_ata: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn book_bettor_place_bet(
    ctx: Context<BookBettorPlaceBetAccounts>,
    odds: u32,
    wager: u64,
    bet_direction: BetDirection,
) -> Result<()> {
    // check condition
    require!(
        ctx.accounts.book_pda.positions.contains_key(ctx.accounts.bettor.key),
        BettingError::UserDidNotOptIn
    );
    require!(wager >= MIN_BET_AMOUNT, BettingError::MinBetAmountNotMet);
    // transfer wager
    let wager_transfer_cpi_context = CpiContext::new(
        ctx.accounts.token_program.to_account_info(),
        anchor_spl::token::Transfer {
            from: ctx.accounts.bettor_token_account.to_account_info(),
            to: ctx.accounts.book_ata.to_account_info(),
            authority: ctx.accounts.bettor.to_account_info(),
        },
    );
    anchor_spl::token::transfer(wager_transfer_cpi_context, wager)?;
    // update book pda
    ctx.accounts
        .book_pda
        .new_bet(odds, wager, ctx.accounts.bettor.key(), bet_direction);

    // realloc
    let book_pda_account_info = ctx.accounts.book_pda.to_account_info();
    book_pda_account_info.realloc(ctx.accounts.book_pda.current_space(), false)?;
    // transfer rent for bet
    let rent_transfer_cpi_context = CpiContext::new(
        ctx.accounts.system_program.to_account_info(),
        system_program::Transfer {
            from: ctx.accounts.bettor.to_account_info(),
            to: ctx.accounts.book_pda.to_account_info(),
        },
    );
    system_program::transfer(rent_transfer_cpi_context, RENT_PER_BET)?;

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

    use crate::state::{BetDirection, BetType, Book, Position};

    #[tokio::test]
    async fn test_book_bettor_place_bet_success() {
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

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 0,
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
            .signer(&bettor)
            .accounts(crate::accounts::BookBettorPlaceBetAccounts {
                bettor: bettor.pubkey(),
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorPlaceBet {
                odds: 1200,
                wager: 1000000 * 20,
                bet_direction: BetDirection::For,
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &bettor],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // wager should be transferred  out from the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 80);
        // book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.bets_count, 1);
        assert_eq!(book_state.wager_total, 1000000 * 20);
        assert_eq!(book_state.bets_for.len(), 1);
        assert_eq!(book_state.bets_for[0].bettor, bettor.pubkey());
        assert_eq!(book_state.bets_for[0].bettor, bettor.pubkey());
        assert_eq!(book_state.bets_for[0].odds(), 1200);
        assert_eq!(book_state.bets_for[0].wager, 1000000 * 20);
        assert_eq!(book_state.positions[&bettor.pubkey()].active_bets_count, 1);
        assert_eq!(book_state.positions[&bettor.pubkey()].bets_count, 1);
        assert_eq!(book_state.positions[&bettor.pubkey()].wager, 1000000 * 20);
        // wager should be transferred to the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 1000000 * 20);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6004)")]
    async fn test_book_bettor_place_bet_err_user_did_not_opt_in() {
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
            positions: BTreeMap::from([]),
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

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 0,
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
            .signer(&bettor)
            .accounts(crate::accounts::BookBettorPlaceBetAccounts {
                bettor: bettor.pubkey(),
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorPlaceBet {
                odds: 1200,
                wager: 1000000 * 20,
                bet_direction: BetDirection::For,
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &bettor],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // wager should be transferred  out from the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 80);
        // book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.bets_count, 1);
        assert_eq!(book_state.wager_total, 1000000 * 20);
        assert_eq!(book_state.bets_for.len(), 1);
        assert_eq!(book_state.bets_for[0].bettor, bettor.pubkey());
        assert_eq!(book_state.bets_for[0].bettor, bettor.pubkey());
        assert_eq!(book_state.bets_for[0].odds(), 1200);
        assert_eq!(book_state.bets_for[0].wager, 1000000 * 20);
        assert_eq!(book_state.positions[&bettor.pubkey()].active_bets_count, 1);
        assert_eq!(book_state.positions[&bettor.pubkey()].bets_count, 1);
        assert_eq!(book_state.positions[&bettor.pubkey()].wager, 1000000 * 20);
        // wager should be transferred to the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 1000000 * 20);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6005)")]
    async fn test_book_bettor_place_bet_err_wager_too_low() {
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

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 0,
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
            .signer(&bettor)
            .accounts(crate::accounts::BookBettorPlaceBetAccounts {
                bettor: bettor.pubkey(),
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookBettorPlaceBet {
                odds: 1200,
                wager: 20,
                bet_direction: BetDirection::For,
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &bettor],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // wager should be transferred  out from the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 80);
        // book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.bets_count, 1);
        assert_eq!(book_state.wager_total, 1000000 * 20);
        assert_eq!(book_state.bets_for.len(), 1);
        assert_eq!(book_state.bets_for[0].bettor, bettor.pubkey());
        assert_eq!(book_state.bets_for[0].bettor, bettor.pubkey());
        assert_eq!(book_state.bets_for[0].odds(), 1200);
        assert_eq!(book_state.bets_for[0].wager, 1000000 * 20);
        assert_eq!(book_state.positions[&bettor.pubkey()].active_bets_count, 1);
        assert_eq!(book_state.positions[&bettor.pubkey()].bets_count, 1);
        assert_eq!(book_state.positions[&bettor.pubkey()].wager, 1000000 * 20);
        // wager should be transferred to the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 1000000 * 20);
    }
}
