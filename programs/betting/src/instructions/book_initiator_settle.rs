use anchor_lang::prelude::*;
use anchor_spl::{
    mint::USDC,
    token::{Token, TokenAccount},
};

use crate::{
    constants::{BETTOR_DISPUTE_WINDOW, BETTOR_PAYOUT_RATE, INITIATOR_REWARD_SHARE, OPERATOR_TOKEN_ACCOUNT},
    error::BettingError,
    state::{Book, UserAccount},
};

#[derive(Accounts)]
pub struct BookInitiatorSettleAccounts<'info> {
    /// CHECK: will be checked by user account and in the instruction
    pub initiator: UncheckedAccount<'info>,
    #[account(mut,seeds=[b"UserAccount".as_ref(),initiator.key().as_ref()],bump)]
    pub initiator_user_account: Account<'info, UserAccount>,
    #[account(mut,token::mint=USDC,token::authority=initiator)]
    pub initiator_token_account: Account<'info, TokenAccount>,
    #[account(mut,seeds=[b"Book".as_ref(),&book_pda.game_id.to_le_bytes(),book_pda.bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
    #[account(mut,associated_token::mint=USDC,associated_token::authority=book_pda)]
    pub book_ata: Account<'info, TokenAccount>,
    #[account(mut,token::mint=USDC,address=OPERATOR_TOKEN_ACCOUNT)]
    pub operator_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

pub fn book_initiator_settle(ctx: Context<BookInitiatorSettleAccounts>) -> Result<()> {
    // check initiator
    require_keys_eq!(
        ctx.accounts.initiator.key(),
        ctx.accounts.book_pda.initiator,
        BettingError::NoAuthority
    );
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
    // oracles should all be settled
    require!(ctx.accounts.book_pda.oracles.is_empty(), BettingError::BookNotSettled);

    // update user account
    ctx.accounts.initiator_user_account.books_initialized -= 1;
    // pay reward
    let total_profit = ctx.accounts.book_pda.dealt_wager * (10000 - BETTOR_PAYOUT_RATE) / 10000;
    let initiator_reward = total_profit * INITIATOR_REWARD_SHARE / 10000;
    let initiator_reward_transfer_cpi_context = CpiContext::new(
        ctx.accounts.token_program.to_account_info(),
        anchor_spl::token::Transfer {
            from: ctx.accounts.book_ata.to_account_info(),
            to: ctx.accounts.initiator_token_account.to_account_info(),
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
        initiator_reward_transfer_cpi_context.with_signer(&[book_pda_signer_seeds]),
        initiator_reward,
    )?;
    // transfer remaining usdc to the operator token account
    ctx.accounts.book_ata.reload().unwrap();
    let operator_profit_transfer_cpi_context = CpiContext::new(
        ctx.accounts.token_program.to_account_info(),
        anchor_spl::token::Transfer {
            from: ctx.accounts.book_ata.to_account_info(),
            to: ctx.accounts.operator_token_account.to_account_info(),
            authority: ctx.accounts.book_pda.to_account_info(),
        },
    );
    anchor_spl::token::transfer(
        operator_profit_transfer_cpi_context.with_signer(&[book_pda_signer_seeds]),
        ctx.accounts.book_ata.amount,
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

    use crate::{
        constants::{BETTOR_DISPUTE_WINDOW, OPERATOR_PUBKEY, OPERATOR_TOKEN_ACCOUNT},
        state::{Bet, BetOutcome, BetType, Book, Oracle, UserAccount},
    };

    #[tokio::test]
    async fn test_book_initiator_settle_success() {
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

        let initiator_ata = anchor_spl::associated_token::get_associated_token_address(&initiator.pubkey(), &USDC);
        let initiator_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: initiator.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut initiator_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(initiator_ata_state, &mut initiator_ata_data).unwrap();
        program_test.add_account(
            initiator_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(initiator_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

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
        let book_pda_state = Book {
            total_oracle_stake: 0,
            game_id,
            initiator: initiator.pubkey(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 1000,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::For),
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
            amount: 1000000 * 2000,
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

        let operator_token_account_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: OPERATOR_PUBKEY,
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut operator_token_account_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(
            operator_token_account_state,
            &mut operator_token_account_data,
        )
        .unwrap();
        program_test.add_account(
            OPERATOR_TOKEN_ACCOUNT,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(operator_token_account_data),
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
            .accounts(crate::accounts::BookInitiatorSettleAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                initiator_token_account: initiator_ata,
                book_pda,
                book_ata,
                operator_token_account: OPERATOR_TOKEN_ACCOUNT,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookInitiatorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // the initiator user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 0);
        // the reward should be transferred to the initiator token account
        let initiator_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(initiator_ata).await.unwrap();
        assert_eq!(initiator_token_account_state.amount, 1000000 * 100 + 1000000 * 2);
        // all the usdc in book ata should be transferred out
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
        // the remaining usdc should all be transferred to the operator token account
        let op_token_account_state: anchor_spl::token::spl_token::state::Account = banks_client
            .get_packed_account_data(OPERATOR_TOKEN_ACCOUNT)
            .await
            .unwrap();
        assert_eq!(op_token_account_state.amount, 1000000 * 100 + 1000000 * 1998);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6000)")]
    async fn test_book_initiator_settle_err_wrong_initiator() {
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

        let initiator_ata = anchor_spl::associated_token::get_associated_token_address(&initiator.pubkey(), &USDC);
        let initiator_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: initiator.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut initiator_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(initiator_ata_state, &mut initiator_ata_data).unwrap();
        program_test.add_account(
            initiator_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(initiator_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

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
        let book_pda_state = Book {
            total_oracle_stake: 0,
            game_id,
            initiator: Pubkey::new_unique(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 1000,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::For),
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
            amount: 1000000 * 2000,
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

        let operator_token_account_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: OPERATOR_PUBKEY,
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut operator_token_account_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(
            operator_token_account_state,
            &mut operator_token_account_data,
        )
        .unwrap();
        program_test.add_account(
            OPERATOR_TOKEN_ACCOUNT,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(operator_token_account_data),
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
            .accounts(crate::accounts::BookInitiatorSettleAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                initiator_token_account: initiator_ata,
                book_pda,
                book_ata,
                operator_token_account: OPERATOR_TOKEN_ACCOUNT,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookInitiatorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // the initiator user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 0);
        // the reward should be transferred to the initiator token account
        let initiator_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(initiator_ata).await.unwrap();
        assert_eq!(initiator_token_account_state.amount, 1000000 * 100 + 1000000 * 2);
        // all the usdc in book ata should be transferred out
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
        // the remaining usdc should all be transferred to the operator token account
        let op_token_account_state: anchor_spl::token::spl_token::state::Account = banks_client
            .get_packed_account_data(OPERATOR_TOKEN_ACCOUNT)
            .await
            .unwrap();
        assert_eq!(op_token_account_state.amount, 1000000 * 100 + 1000000 * 1998);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_initiator_settle_err_not_concluded() {
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

        let initiator_ata = anchor_spl::associated_token::get_associated_token_address(&initiator.pubkey(), &USDC);
        let initiator_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: initiator.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut initiator_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(initiator_ata_state, &mut initiator_ata_data).unwrap();
        program_test.add_account(
            initiator_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(initiator_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

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
        let book_pda_state = Book {
            total_oracle_stake: 0,
            game_id,
            initiator: initiator.pubkey(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 1000,
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
            amount: 1000000 * 2000,
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

        let operator_token_account_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: OPERATOR_PUBKEY,
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut operator_token_account_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(
            operator_token_account_state,
            &mut operator_token_account_data,
        )
        .unwrap();
        program_test.add_account(
            OPERATOR_TOKEN_ACCOUNT,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(operator_token_account_data),
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
            .accounts(crate::accounts::BookInitiatorSettleAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                initiator_token_account: initiator_ata,
                book_pda,
                book_ata,
                operator_token_account: OPERATOR_TOKEN_ACCOUNT,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookInitiatorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // the initiator user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 0);
        // the reward should be transferred to the initiator token account
        let initiator_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(initiator_ata).await.unwrap();
        assert_eq!(initiator_token_account_state.amount, 1000000 * 100 + 1000000 * 2);
        // all the usdc in book ata should be transferred out
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
        // the remaining usdc should all be transferred to the operator token account
        let op_token_account_state: anchor_spl::token::spl_token::state::Account = banks_client
            .get_packed_account_data(OPERATOR_TOKEN_ACCOUNT)
            .await
            .unwrap();
        assert_eq!(op_token_account_state.amount, 1000000 * 100 + 1000000 * 1998);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6006)")]
    async fn test_book_initiator_settle_err_bettor_dispute_window_not_passed() {
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

        let initiator_ata = anchor_spl::associated_token::get_associated_token_address(&initiator.pubkey(), &USDC);
        let initiator_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: initiator.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut initiator_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(initiator_ata_state, &mut initiator_ata_data).unwrap();
        program_test.add_account(
            initiator_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(initiator_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

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
        let book_pda_state = Book {
            total_oracle_stake: 0,
            game_id,
            initiator: initiator.pubkey(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 1000,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(chrono::Utc::now().timestamp() - BETTOR_DISPUTE_WINDOW + 60),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::For),
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
            amount: 1000000 * 2000,
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

        let operator_token_account_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: OPERATOR_PUBKEY,
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut operator_token_account_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(
            operator_token_account_state,
            &mut operator_token_account_data,
        )
        .unwrap();
        program_test.add_account(
            OPERATOR_TOKEN_ACCOUNT,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(operator_token_account_data),
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
            .accounts(crate::accounts::BookInitiatorSettleAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                initiator_token_account: initiator_ata,
                book_pda,
                book_ata,
                operator_token_account: OPERATOR_TOKEN_ACCOUNT,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookInitiatorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // the initiator user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 0);
        // the reward should be transferred to the initiator token account
        let initiator_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(initiator_ata).await.unwrap();
        assert_eq!(initiator_token_account_state.amount, 1000000 * 100 + 1000000 * 2);
        // all the usdc in book ata should be transferred out
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
        // the remaining usdc should all be transferred to the operator token account
        let op_token_account_state: anchor_spl::token::spl_token::state::Account = banks_client
            .get_packed_account_data(OPERATOR_TOKEN_ACCOUNT)
            .await
            .unwrap();
        assert_eq!(op_token_account_state.amount, 1000000 * 100 + 1000000 * 1998);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6002)")]
    async fn test_book_initiator_settle_err_bettors_not_settled() {
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

        let initiator_ata = anchor_spl::associated_token::get_associated_token_address(&initiator.pubkey(), &USDC);
        let initiator_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: initiator.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut initiator_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(initiator_ata_state, &mut initiator_ata_data).unwrap();
        program_test.add_account(
            initiator_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(initiator_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

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
        let book_pda_state = Book {
            total_oracle_stake: 0,
            game_id,
            initiator: initiator.pubkey(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 1000,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::new(),
            bets_for: VecDeque::from([Bet {
                id: 0,
                bettor: Pubkey::new_unique(),
                wager: 1000000 * 100,
            }]),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::For),
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
            amount: 1000000 * 2000,
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

        let operator_token_account_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: OPERATOR_PUBKEY,
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut operator_token_account_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(
            operator_token_account_state,
            &mut operator_token_account_data,
        )
        .unwrap();
        program_test.add_account(
            OPERATOR_TOKEN_ACCOUNT,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(operator_token_account_data),
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
            .accounts(crate::accounts::BookInitiatorSettleAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                initiator_token_account: initiator_ata,
                book_pda,
                book_ata,
                operator_token_account: OPERATOR_TOKEN_ACCOUNT,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookInitiatorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // the initiator user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 0);
        // the reward should be transferred to the initiator token account
        let initiator_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(initiator_ata).await.unwrap();
        assert_eq!(initiator_token_account_state.amount, 1000000 * 100 + 1000000 * 2);
        // all the usdc in book ata should be transferred out
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
        // the remaining usdc should all be transferred to the operator token account
        let op_token_account_state: anchor_spl::token::spl_token::state::Account = banks_client
            .get_packed_account_data(OPERATOR_TOKEN_ACCOUNT)
            .await
            .unwrap();
        assert_eq!(op_token_account_state.amount, 1000000 * 100 + 1000000 * 1998);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6002)")]
    async fn test_book_initiator_settle_err_oracles_not_settled() {
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

        let initiator_ata = anchor_spl::associated_token::get_associated_token_address(&initiator.pubkey(), &USDC);
        let initiator_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: initiator.pubkey(),
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut initiator_ata_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(initiator_ata_state, &mut initiator_ata_data).unwrap();
        program_test.add_account(
            initiator_ata,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(initiator_ata_data),
                owner: anchor_spl::token::ID,
                ..Default::default()
            },
        );

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
        let book_pda_state = Book {
            total_oracle_stake: 0,
            game_id,
            initiator: initiator.pubkey(),
            bets_count: 0,
            wager_total: 0,
            payout_for_total: 0,
            payout_against_total: 0,
            dealt_wager: 1000000 * 1000,
            bet_type,
            total_dispute_stake: 0,
            dispute_resolution_result: None,
            concluded_at: Some(0),
            oracles: BTreeMap::from([(
                Pubkey::new_unique(),
                Oracle {
                    stake: 1000000 * 100,
                    outcome: None,
                },
            )]),
            bets_for: VecDeque::new(),
            bets_against: VecDeque::new(),
            positions: BTreeMap::new(),
            aggregated_oracle_outcome: Some(BetOutcome::For),
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
            amount: 1000000 * 2000,
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

        let operator_token_account_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: OPERATOR_PUBKEY,
            amount: 1000000 * 100,
            state: anchor_spl::token::spl_token::state::AccountState::Initialized,
            ..Default::default()
        };
        let mut operator_token_account_data = [0_u8; 165];
        anchor_spl::token::spl_token::state::Account::pack(
            operator_token_account_state,
            &mut operator_token_account_data,
        )
        .unwrap();
        program_test.add_account(
            OPERATOR_TOKEN_ACCOUNT,
            Account {
                lamports: Rent::default().minimum_balance(165),
                data: Vec::from(operator_token_account_data),
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
            .accounts(crate::accounts::BookInitiatorSettleAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                initiator_token_account: initiator_ata,
                book_pda,
                book_ata,
                operator_token_account: OPERATOR_TOKEN_ACCOUNT,
                token_program: anchor_spl::token::ID,
            })
            .args(crate::instruction::BookInitiatorSettle)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(&instructions, Some(&payer.pubkey()), &[&payer], recent_blockhash);
        banks_client.process_transaction(tx).await.unwrap();

        // the initiator user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 0);
        // the reward should be transferred to the initiator token account
        let initiator_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(initiator_ata).await.unwrap();
        assert_eq!(initiator_token_account_state.amount, 1000000 * 100 + 1000000 * 2);
        // all the usdc in book ata should be transferred out
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.amount, 0);
        // the remaining usdc should all be transferred to the operator token account
        let op_token_account_state: anchor_spl::token::spl_token::state::Account = banks_client
            .get_packed_account_data(OPERATOR_TOKEN_ACCOUNT)
            .await
            .unwrap();
        assert_eq!(op_token_account_state.amount, 1000000 * 100 + 1000000 * 1998);
    }
}
