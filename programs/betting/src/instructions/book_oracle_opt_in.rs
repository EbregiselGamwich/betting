use anchor_lang::{prelude::*, system_program};
use anchor_spl::{
    mint::USDC,
    token::{Token, TokenAccount},
};

use crate::{
    constants::{MIN_ORACLE_STAKE, RENT_PER_ORACLE},
    error::BettingError,
    state::{user_account::UserAccount, Book, Oracle},
};

#[derive(Accounts)]
pub struct BookOracleOptInAccounts<'info> {
    #[account(mut)]
    pub oracle: Signer<'info>,
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

pub fn book_oracle_opt_in(ctx: Context<BookOracleOptInAccounts>, stake: u64) -> Result<()> {
    // check stake
    require!(stake >= MIN_ORACLE_STAKE, BettingError::MinTokenAmountNotMet);
    // update user account
    match ctx
        .accounts
        .oracle_user_account
        .books_oracled
        .binary_search(&ctx.accounts.book_pda.key())
    {
        Ok(_) => {
            return err!(BettingError::UserAlreadyOptIn);
        }
        Err(index) => {
            // add book pda
            ctx.accounts
                .oracle_user_account
                .books_oracled
                .insert(index, ctx.accounts.book_pda.key());
            // check space
            let user_account_space = ctx.accounts.oracle_user_account.current_space();
            let oracle_pda_account_info = ctx.accounts.oracle_user_account.to_account_info();
            if oracle_pda_account_info.data_len() < user_account_space {
                oracle_pda_account_info.realloc(user_account_space, false)?;
            }
            // check rent
            let min_rent = Rent::get()?.minimum_balance(user_account_space);
            if oracle_pda_account_info.lamports() < min_rent {
                let diff = min_rent - oracle_pda_account_info.lamports();
                let transfer_cpi_context = CpiContext::new(
                    ctx.accounts.system_program.to_account_info(),
                    system_program::Transfer {
                        from: ctx.accounts.oracle.to_account_info(),
                        to: ctx.accounts.oracle_user_account.to_account_info(),
                    },
                );
                system_program::transfer(transfer_cpi_context, diff)?;
            }
        }
    }
    // transfer stake
    let stake_transfer_cpi_context = CpiContext::new(
        ctx.accounts.token_program.to_account_info(),
        anchor_spl::token::Transfer {
            from: ctx.accounts.oracle_token_account.to_account_info(),
            to: ctx.accounts.book_ata.to_account_info(),
            authority: ctx.accounts.oracle.to_account_info(),
        },
    );
    anchor_spl::token::transfer(stake_transfer_cpi_context, stake)?;
    // update book pda
    match ctx.accounts.book_pda.oracles.get(ctx.accounts.oracle.key) {
        Some(_) => {
            return err!(BettingError::UserAlreadyOptIn);
        }
        None => {
            ctx.accounts
                .book_pda
                .oracles
                .insert(ctx.accounts.oracle.key(), Oracle { stake, outcome: None });

            // realloc
            let book_pda_account_info = ctx.accounts.book_pda.to_account_info();
            let book_pda_space = ctx.accounts.book_pda.current_space();
            book_pda_account_info.realloc(book_pda_space, false)?;
            // rent
            let transfer_cpi_context = CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                system_program::Transfer {
                    from: ctx.accounts.oracle.to_account_info(),
                    to: ctx.accounts.book_pda.to_account_info(),
                },
            );
            system_program::transfer(transfer_cpi_context, RENT_PER_ORACLE)?;
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
    use anchor_spl::mint::USDC;
    use solana_program_test::{tokio, ProgramTest};
    use solana_sdk::{
        account::Account, native_token::LAMPORTS_PER_SOL, program_pack::Pack, pubkey::Pubkey, rent::Rent,
        signature::Keypair, signer::Signer, system_program, transaction::Transaction,
    };

    use crate::state::{user_account::UserAccount, BetType, Book, Oracle};

    #[tokio::test]
    async fn test_book_oracle_opt_in_success() {
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

        let (oracle_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), oracle.pubkey().as_ref()], &program_id);
        let oracle_pda_state = UserAccount {
            authority: oracle.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::new(),
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
            .signer(&oracle)
            .accounts(crate::accounts::BookOracleOptInAccounts {
                oracle: oracle.pubkey(),
                oracle_user_account: oracle_pda,
                oracle_token_account: oracle_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookOracleOptIn { stake: 1000000 * 20 })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &oracle],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be saved to the oracle user account
        let user_account = banks_client.get_account(oracle_pda).await.unwrap().unwrap();
        let user_account_state = UserAccount::try_deserialize(&mut user_account.data.as_slice()).unwrap();
        assert!(user_account_state.books_oracled.contains(&book_pda));
        // oracle stake should be transferred from the oracle token account
        let user_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(oracle_ata).await.unwrap();
        assert_eq!(user_token_account_state.amount, 1000000 * 80);
        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.oracles.contains_key(&oracle.pubkey()));
        assert!(book_state.oracles[&oracle.pubkey()].outcome.is_none());
        assert_eq!(book_state.oracles[&oracle.pubkey()].stake, 1000000 * 20);
        // the stake should be transferred to the book ata
        let book_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_token_account_state.amount, 1000000 * 20);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6005")]
    async fn test_book_oracle_opt_in_err_min_stake_not_met() {
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

        let (oracle_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), oracle.pubkey().as_ref()], &program_id);
        let oracle_pda_state = UserAccount {
            authority: oracle.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::new(),
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
            .signer(&oracle)
            .accounts(crate::accounts::BookOracleOptInAccounts {
                oracle: oracle.pubkey(),
                oracle_user_account: oracle_pda,
                oracle_token_account: oracle_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookOracleOptIn { stake: 20 })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &oracle],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be saved to the oracle user account
        let user_account = banks_client.get_account(oracle_pda).await.unwrap().unwrap();
        let user_account_state = UserAccount::try_deserialize(&mut user_account.data.as_slice()).unwrap();
        assert!(user_account_state.books_oracled.contains(&book_pda));
        // oracle stake should be transferred from the oracle token account
        let user_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(oracle_ata).await.unwrap();
        assert_eq!(user_token_account_state.amount, 1000000 * 80);
        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.oracles.contains_key(&oracle.pubkey()));
        assert!(book_state.oracles[&oracle.pubkey()].outcome.is_none());
        assert_eq!(book_state.oracles[&oracle.pubkey()].stake, 1000000 * 20);
        // the stake should be transferred to the book ata
        let book_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_token_account_state.amount, 1000000 * 20);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6003)")]
    async fn test_book_oracle_opt_in_err_already_opt_in() {
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
        let mut book_pda_state = Book {
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
        book_pda_state.oracles.insert(
            oracle.pubkey(),
            Oracle {
                stake: 1000000 * 10,
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

        let (oracle_pda, _) =
            Pubkey::find_program_address(&[b"UserAccount".as_ref(), oracle.pubkey().as_ref()], &program_id);
        let oracle_pda_state = UserAccount {
            authority: oracle.pubkey(),
            books_initialized: 0,
            books_oracled: VecDeque::from(vec![book_pda]),
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
        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);
        let book_ata_state = anchor_spl::token::spl_token::state::Account {
            mint: USDC,
            owner: book_pda,
            amount: 1000000 * 10,
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
            .signer(&oracle)
            .accounts(crate::accounts::BookOracleOptInAccounts {
                oracle: oracle.pubkey(),
                oracle_user_account: oracle_pda,
                oracle_token_account: oracle_ata,
                book_pda,
                book_ata,
                token_program: anchor_spl::token::ID,
                system_program: system_program::id(),
            })
            .args(crate::instruction::BookOracleOptIn { stake: 1000000 * 20 })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &oracle],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the book pda should be saved to the oracle user account
        let user_account = banks_client.get_account(oracle_pda).await.unwrap().unwrap();
        let user_account_state = UserAccount::try_deserialize(&mut user_account.data.as_slice()).unwrap();
        assert!(user_account_state.books_oracled.contains(&book_pda));
        // oracle stake should be transferred from the oracle token account
        let user_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(oracle_ata).await.unwrap();
        assert_eq!(user_token_account_state.amount, 1000000 * 80);
        // the book pda should be updated
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert!(book_state.oracles.contains_key(&oracle.pubkey()));
        assert!(book_state.oracles[&oracle.pubkey()].outcome.is_none());
        assert_eq!(book_state.oracles[&oracle.pubkey()].stake, 1000000 * 20);
        // the stake should be transferred to the book ata
        let book_token_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_token_account_state.amount, 1000000 * 20);
    }
}
