use anchor_lang::prelude::*;
use anchor_spl::{
    mint::USDC,
    token::{Token, TokenAccount},
};

use crate::{
    constants::ORACLE_UPDATE_WINDOW,
    error::BettingError,
    state::{BetDirection, Book},
};

#[derive(Accounts)]
pub struct BookBettorCancelBetAccounts<'info> {
    pub bettor: Signer<'info>,
    #[account(mut,token::mint=USDC,token::authority=bettor)]
    pub bettor_token_account: Account<'info, TokenAccount>,
    #[account(mut,seeds=[b"Book".as_ref(),&book_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
    #[account(mut,associated_token::mint=USDC,associated_token::authority=book_pda)]
    pub book_ata: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

pub fn book_bettor_cancel_bet(
    ctx: Context<BookBettorCancelBetAccounts>,
    bet_id: u64,
    bet_direction: BetDirection,
) -> Result<()> {
    // check window
    let now = Clock::get()?.unix_timestamp;
    require!(
        ctx.accounts.book_pda.concluded_at.is_none()
            || ctx.accounts.book_pda.concluded_at.unwrap() + ORACLE_UPDATE_WINDOW > now,
        BettingError::NotInWindow
    );
    // get the bet
    let bets = if let BetDirection::For = bet_direction {
        &mut ctx.accounts.book_pda.bets_for
    } else {
        &mut ctx.accounts.book_pda.bets_against
    };
    let bet = if let Ok(index) = bets.binary_search_by_key(&bet_id, |b| b.id) {
        bets.remove(index).unwrap()
    } else {
        return err!(BettingError::NotFound);
    };
    // check bettor
    require_keys_eq!(bet.bettor, ctx.accounts.bettor.key(), BettingError::NoAuthority);
    // update book pda
    ctx.accounts.book_pda.wager_total -= bet.wager;
    ctx.accounts.book_pda.positions.entry(bet.bettor).and_modify(|p| {
        p.active_bets_count -= 1;
        p.wager -= bet.wager;
    });

    // return the wager
    let wager_return_cpi_context = CpiContext::new(
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
        wager_return_cpi_context.with_signer(&[book_pda_signer_seeds]),
        bet.wager,
    )?;

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

    use crate::state::{BetDirection, BetType, Book, Position};

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_bettor_cancel_bet_err_cancel_after_conclusion() {
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
        let mut book_pda_state = Book {
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
        book_pda_state.new_bet(1200, 1000000 * 20, bettor.pubkey(), BetDirection::For);
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
            amount: 1000000 * 20,
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
            .accounts(crate::accounts::BookBettorCancelBetAccounts {
                bettor: bettor.pubkey(),
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookBettorCancelBet {
                bet_id: book_pda_state.bets_for[0].id,
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

        // the wager should be returned to the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 120);
        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.wager_total, 0);
        assert!(book_state.bets_for.is_empty());
        assert_eq!(book_state.positions[&bettor.pubkey()].wager, 0);
        assert_eq!(book_state.positions[&bettor.pubkey()].active_bets_count, 0);
        assert_eq!(book_state.positions[&bettor.pubkey()].bets_count, 1);
        // the wager should be transferred from the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
    }

    #[tokio::test]
    async fn test_book_bettor_cancel_bet_success() {
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
        let mut book_pda_state = Book {
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
        book_pda_state.new_bet(1200, 1000000 * 20, bettor.pubkey(), BetDirection::For);
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
            amount: 1000000 * 20,
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
            .accounts(crate::accounts::BookBettorCancelBetAccounts {
                bettor: bettor.pubkey(),
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookBettorCancelBet {
                bet_id: book_pda_state.bets_for[0].id,
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

        // the wager should be returned to the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 120);
        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.wager_total, 0);
        assert!(book_state.bets_for.is_empty());
        assert_eq!(book_state.positions[&bettor.pubkey()].wager, 0);
        assert_eq!(book_state.positions[&bettor.pubkey()].active_bets_count, 0);
        assert_eq!(book_state.positions[&bettor.pubkey()].bets_count, 1);
        // the wager should be transferred from the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6007)")]
    async fn test_book_bettor_cancel_bet_err_bet_not_found() {
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
        let mut book_pda_state = Book {
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
        book_pda_state.new_bet(1200, 1000000 * 20, bettor.pubkey(), BetDirection::For);
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
            amount: 1000000 * 20,
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
            .accounts(crate::accounts::BookBettorCancelBetAccounts {
                bettor: bettor.pubkey(),
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookBettorCancelBet {
                bet_id: book_pda_state.bets_for[0].id,
                bet_direction: BetDirection::Against,
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

        // the wager should be returned to the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 120);
        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.wager_total, 0);
        assert!(book_state.bets_for.is_empty());
        assert_eq!(book_state.positions[&bettor.pubkey()].wager, 0);
        assert_eq!(book_state.positions[&bettor.pubkey()].active_bets_count, 0);
        assert_eq!(book_state.positions[&bettor.pubkey()].bets_count, 1);
        // the wager should be transferred from the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6000)")]
    async fn test_book_bettor_cancel_bet_err_wrong_bettor() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let wrong_bettor = Pubkey::new_unique();
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
        let mut book_pda_state = Book {
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
                wrong_bettor,
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
        book_pda_state.new_bet(1200, 1000000 * 20, wrong_bettor, BetDirection::For);
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
            amount: 1000000 * 20,
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
            .accounts(crate::accounts::BookBettorCancelBetAccounts {
                bettor: bettor.pubkey(),
                bettor_token_account: bettor_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookBettorCancelBet {
                bet_id: book_pda_state.bets_for[0].id,
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

        // the wager should be returned to the bettor token account
        let bettor_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(bettor_ata).await.unwrap();
        assert_eq!(bettor_token_account_state.amount, 1000000 * 120);
        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.wager_total, 0);
        assert!(book_state.bets_for.is_empty());
        assert_eq!(book_state.positions[&bettor.pubkey()].wager, 0);
        assert_eq!(book_state.positions[&bettor.pubkey()].active_bets_count, 0);
        assert_eq!(book_state.positions[&bettor.pubkey()].bets_count, 1);
        // the wager should be transferred from the book ata
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
    }
}
