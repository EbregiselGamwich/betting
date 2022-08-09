use anchor_lang::prelude::*;
use anchor_spl::{
    mint::USDC,
    token::{Token, TokenAccount},
};

use crate::{
    error::BettingError,
    state::{game::Game, user_account::UserAccount, Book},
};

#[derive(Accounts)]
pub struct BookCloseAccounts<'info> {
    #[account(mut)]
    pub initiator: Signer<'info>,
    #[account(mut,seeds=[b"UserAccount".as_ref(),initiator.key().as_ref()],bump)]
    pub initiator_user_account: Account<'info, UserAccount>,
    #[account(mut,seeds=[b"Game".as_ref(),&game_pda.game_id.to_le_bytes()],bump)]
    pub game_pda: Account<'info, Game>,
    #[account(mut,close=initiator,seeds=[b"Book".as_ref(),&game_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
    #[account(mut,associated_token::mint=USDC,associated_token::authority=book_pda)]
    pub book_ata: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

pub fn book_close(ctx: Context<BookCloseAccounts>) -> Result<()> {
    // check initiator
    require_keys_eq!(
        ctx.accounts.initiator.key(),
        ctx.accounts.book_pda.initiator,
        BettingError::NoAuthority
    );
    // check whether the book is settled
    require!(
        ctx.accounts.book_pda.bets_for.is_empty()
            && ctx.accounts.book_pda.bets_against.is_empty()
            && ctx.accounts.book_pda.oracles.is_empty()
            && ctx.accounts.book_pda.positions.is_empty()
            && ctx.accounts.book_ata.amount == 0,
        BettingError::BookNotSettled
    );
    // update user account
    ctx.accounts.initiator_user_account.books_initialized -= 1;
    // update game pda account
    ctx.accounts.game_pda.books_count -= 1;
    // close book ata
    let bet_type_vec = ctx.accounts.book_pda.bet_type.try_to_vec().unwrap();
    let book_pda_signer_seeds = &[
        b"Book".as_ref(),
        &ctx.accounts.book_pda.game_id.to_le_bytes(),
        bet_type_vec.as_slice(),
        &[*ctx.bumps.get("book_pda").unwrap()],
    ];
    let book_ata_close_cpi_context = CpiContext::new(
        ctx.accounts.token_program.to_account_info(),
        anchor_spl::token::CloseAccount {
            account: ctx.accounts.book_ata.to_account_info(),
            destination: ctx.accounts.initiator.to_account_info(),
            authority: ctx.accounts.book_pda.to_account_info(),
        },
    );
    anchor_spl::token::close_account(book_ata_close_cpi_context.with_signer(&[book_pda_signer_seeds]))?;

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
        signature::Keypair, signer::Signer, transaction::Transaction,
    };

    use crate::state::{game::Game, user_account::UserAccount, BetType, Book};

    #[tokio::test]
    async fn test_book_close_success() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let initiator = Keypair::new();
        program_test.add_account(
            initiator.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let (initiator_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), initiator.pubkey().as_ref()], &program_id);
        let initiator_pda_state = UserAccount {
            authority: initiator.pubkey(),
            books_initialized: 1,
            books_oracled: VecDeque::new(),
            books_bet_on: VecDeque::new(),
        };
        let mut initiator_pda_data: Vec<u8> = Vec::new();
        initiator_pda_state.try_serialize(&mut initiator_pda_data).unwrap();
        program_test.add_account(
            initiator_pda,
            Account {
                lamports: Rent::default().minimum_balance(initiator_pda_state.current_space()),
                data: initiator_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let game_id: u32 = 1;
        let league_id: u32 = 1;
        let home_team_id: u32 = 1;
        let away_team_id: u32 = 1;
        let kickoff: i64 = 1;
        let (game_pda, _) = Pubkey::find_program_address(&[b"Game".as_ref(), &game_id.to_le_bytes()], &program_id);
        let game_pda_state = Game {
            game_id,
            league_id,
            home_team_id,
            away_team_id,
            kickoff,
            books_count: 1,
        };
        let mut game_pda_data: Vec<u8> = Vec::new();
        game_pda_state.try_serialize(&mut game_pda_data).unwrap();
        program_test.add_account(
            game_pda,
            Account {
                lamports: Rent::default().minimum_balance(Game::INIT_SPACE),
                data: game_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

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
            initiator: initiator.pubkey(),
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
            .signer(&initiator)
            .accounts(crate::accounts::BookCloseAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                game_pda,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookClose)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &initiator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the rent from the book pda and the book ata accounts should be returned to the initiator system account
        let initiator_system_account = banks_client.get_account(initiator.pubkey()).await.unwrap().unwrap();
        assert_eq!(
            initiator_system_account.lamports,
            LAMPORTS_PER_SOL
                + Rent::default().minimum_balance(book_pda_state.current_space())
                + Rent::default().minimum_balance(165)
        );
        // the user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 0);
        // the game pda account should be updated
        let game_account = banks_client.get_account(game_pda).await.unwrap().unwrap();
        let game_state = Game::try_deserialize(&mut game_account.data.as_slice()).unwrap();
        assert_eq!(game_state.books_count, 0);
        // the book pda account should be closed
        assert!(banks_client.get_account(book_pda).await.unwrap().is_none());
        // the book ata account should be closed
        assert!(banks_client.get_account(book_ata).await.unwrap().is_none());
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6000)")] // no authority
    async fn test_book_close_err_wrong_initiator() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let initiator = Keypair::new();
        program_test.add_account(
            initiator.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let (initiator_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), initiator.pubkey().as_ref()], &program_id);
        let initiator_pda_state = UserAccount {
            authority: initiator.pubkey(),
            books_initialized: 1,
            books_oracled: VecDeque::new(),
            books_bet_on: VecDeque::new(),
        };
        let mut initiator_pda_data: Vec<u8> = Vec::new();
        initiator_pda_state.try_serialize(&mut initiator_pda_data).unwrap();
        program_test.add_account(
            initiator_pda,
            Account {
                lamports: Rent::default().minimum_balance(initiator_pda_state.current_space()),
                data: initiator_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let game_id: u32 = 1;
        let league_id: u32 = 1;
        let home_team_id: u32 = 1;
        let away_team_id: u32 = 1;
        let kickoff: i64 = 1;
        let (game_pda, _) = Pubkey::find_program_address(&[b"Game".as_ref(), &game_id.to_le_bytes()], &program_id);
        let game_pda_state = Game {
            game_id,
            league_id,
            home_team_id,
            away_team_id,
            kickoff,
            books_count: 1,
        };
        let mut game_pda_data: Vec<u8> = Vec::new();
        game_pda_state.try_serialize(&mut game_pda_data).unwrap();
        program_test.add_account(
            game_pda,
            Account {
                lamports: Rent::default().minimum_balance(Game::INIT_SPACE),
                data: game_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

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
            .signer(&initiator)
            .accounts(crate::accounts::BookCloseAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                game_pda,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookClose)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &initiator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the rent from the book pda and the book ata accounts should be returned to the initiator system account
        let initiator_system_account = banks_client.get_account(initiator.pubkey()).await.unwrap().unwrap();
        assert_eq!(
            initiator_system_account.lamports,
            LAMPORTS_PER_SOL
                + Rent::default().minimum_balance(book_pda_state.current_space())
                + Rent::default().minimum_balance(165)
        );
        // the user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 0);
        // the game pda account should be updated
        let game_account = banks_client.get_account(game_pda).await.unwrap().unwrap();
        let game_state = Game::try_deserialize(&mut game_account.data.as_slice()).unwrap();
        assert_eq!(game_state.books_count, 0);
        // the book pda account should be closed
        assert!(banks_client.get_account(book_pda).await.unwrap().is_none());
        // the book ata account should be closed
        assert!(banks_client.get_account(book_ata).await.unwrap().is_none());
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6002)")]
    async fn test_book_close_err_book_not_settled() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let initiator = Keypair::new();
        program_test.add_account(
            initiator.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let (initiator_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), initiator.pubkey().as_ref()], &program_id);
        let initiator_pda_state = UserAccount {
            authority: initiator.pubkey(),
            books_initialized: 1,
            books_oracled: VecDeque::new(),
            books_bet_on: VecDeque::new(),
        };
        let mut initiator_pda_data: Vec<u8> = Vec::new();
        initiator_pda_state.try_serialize(&mut initiator_pda_data).unwrap();
        program_test.add_account(
            initiator_pda,
            Account {
                lamports: Rent::default().minimum_balance(initiator_pda_state.current_space()),
                data: initiator_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

        let game_id: u32 = 1;
        let league_id: u32 = 1;
        let home_team_id: u32 = 1;
        let away_team_id: u32 = 1;
        let kickoff: i64 = 1;
        let (game_pda, _) = Pubkey::find_program_address(&[b"Game".as_ref(), &game_id.to_le_bytes()], &program_id);
        let game_pda_state = Game {
            game_id,
            league_id,
            home_team_id,
            away_team_id,
            kickoff,
            books_count: 1,
        };
        let mut game_pda_data: Vec<u8> = Vec::new();
        game_pda_state.try_serialize(&mut game_pda_data).unwrap();
        program_test.add_account(
            game_pda,
            Account {
                lamports: Rent::default().minimum_balance(Game::INIT_SPACE),
                data: game_pda_data,
                owner: program_id,
                ..Default::default()
            },
        );

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
            initiator: initiator.pubkey(),
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

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 100,
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
            .signer(&initiator)
            .accounts(crate::accounts::BookCloseAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                game_pda,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookClose)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &initiator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the rent from the book pda and the book ata accounts should be returned to the initiator system account
        let initiator_system_account = banks_client.get_account(initiator.pubkey()).await.unwrap().unwrap();
        assert_eq!(
            initiator_system_account.lamports,
            LAMPORTS_PER_SOL
                + Rent::default().minimum_balance(book_pda_state.current_space())
                + Rent::default().minimum_balance(165)
        );
        // the user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 0);
        // the game pda account should be updated
        let game_account = banks_client.get_account(game_pda).await.unwrap().unwrap();
        let game_state = Game::try_deserialize(&mut game_account.data.as_slice()).unwrap();
        assert_eq!(game_state.books_count, 0);
        // the book pda account should be closed
        assert!(banks_client.get_account(book_pda).await.unwrap().is_none());
        // the book ata account should be closed
        assert!(banks_client.get_account(book_ata).await.unwrap().is_none());
    }
}
