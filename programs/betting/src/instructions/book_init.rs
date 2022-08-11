use std::collections::{BTreeMap, VecDeque};

use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    mint::USDC,
    token::{Mint, Token, TokenAccount},
};

use crate::state::{BetType, Book, Game, UserAccount};

#[derive(Accounts)]
#[instruction(bet_type:BetType)]
pub struct BookInitAccounts<'info> {
    #[account(mut)]
    pub initiator: Signer<'info>,
    #[account(mut,seeds=[b"UserAccount".as_ref(),initiator.key().as_ref()],bump)]
    pub initiator_user_account: Account<'info, UserAccount>,
    #[account(mut,seeds=[b"Game".as_ref(),&game_pda.game_id.to_le_bytes()],bump)]
    pub game_pda: Account<'info, Game>,
    #[account(init,payer=initiator,space=Book::INIT_SPACE,seeds=[b"Book".as_ref(),&game_pda.game_id.to_le_bytes(),bet_type.try_to_vec().unwrap().as_slice()],bump)]
    pub book_pda: Account<'info, Book>,
    #[account(init,payer=initiator,associated_token::mint=usdc_mint,associated_token::authority=book_pda)]
    pub book_ata: Account<'info, TokenAccount>,
    #[account(address=USDC)]
    pub usdc_mint: Account<'info, Mint>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn book_init(ctx: Context<BookInitAccounts>, bet_type: BetType) -> Result<()> {
    // update initiator user account
    ctx.accounts.initiator_user_account.books_initialized += 1;
    // update game pda
    ctx.accounts.game_pda.books_count += 1;
    // init book pda
    ctx.accounts.book_pda.set_inner(Book {
        total_oracle_stake: 0,
        game_id: ctx.accounts.game_pda.game_id,
        initiator: ctx.accounts.initiator.key(),
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
    });

    Ok(())
}

#[cfg(test)]
mod test {
    use std::{collections::VecDeque, rc::Rc, str::FromStr};

    use anchor_client::RequestBuilder;
    use anchor_lang::{AccountDeserialize, AccountSerialize, AnchorSerialize};
    use anchor_spl::mint::USDC;
    use solana_program_test::{tokio, ProgramTest};
    use solana_sdk::{
        account::Account, native_token::LAMPORTS_PER_SOL, program_pack::Pack, pubkey::Pubkey, rent::Rent,
        signature::Keypair, signer::Signer, system_program, transaction::Transaction,
    };

    use crate::state::{game::Game, user_account::UserAccount, BetType, Book};

    #[tokio::test]
    async fn test_book_init_success() {
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
            books_initialized: 0,
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
            books_count: 0,
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

        let book_ata = anchor_spl::associated_token::get_associated_token_address(&book_pda, &USDC);

        let usdc_mint_state = anchor_spl::token::spl_token::state::Mint {
            supply: u64::MAX,
            decimals: 6,
            is_initialized: true,
            ..Default::default()
        };
        let mut usdc_mint_data = [0_u8; 82];
        anchor_spl::token::spl_token::state::Mint::pack(usdc_mint_state, &mut usdc_mint_data).unwrap();
        program_test.add_account(
            USDC,
            Account {
                lamports: Rent::default().minimum_balance(82),
                data: Vec::from(usdc_mint_data),
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
            .accounts(crate::accounts::BookInitAccounts {
                initiator: initiator.pubkey(),
                initiator_user_account: initiator_pda,
                game_pda,
                book_pda,
                book_ata,
                usdc_mint: USDC,
                token_program: anchor_spl::token::ID,
                associated_token_program: anchor_spl::associated_token::ID,
                system_program: system_program::id(),
                rent: Pubkey::from_str("SysvarRent111111111111111111111111111111111").unwrap(),
            })
            .args(crate::instruction::BookInit { bet_type })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &initiator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the user account should be updated
        let initiator_user_account = banks_client.get_account(initiator_pda).await.unwrap().unwrap();
        let initiator_user_account_state =
            UserAccount::try_deserialize(&mut initiator_user_account.data.as_slice()).unwrap();
        assert_eq!(initiator_user_account_state.books_initialized, 1);
        // the game pda account should be updated
        let game_account = banks_client.get_account(game_pda).await.unwrap().unwrap();
        let game_state = Game::try_deserialize(&mut game_account.data.as_slice()).unwrap();
        assert_eq!(game_state.books_count, 1);
        // the book pda account should be created
        let book_account = banks_client.get_account(book_pda).await.unwrap().unwrap();
        let book_state = Book::try_deserialize(&mut book_account.data.as_slice()).unwrap();
        assert_eq!(book_state.game_id, game_id);
        assert_eq!(book_state.initiator, initiator.pubkey());
        assert_eq!(book_state.bets_count, 0);
        assert_eq!(book_state.wager_total, 0);
        assert_eq!(book_state.payout_for_total, 0);
        assert_eq!(book_state.payout_against_total, 0);
        assert_eq!(book_state.dealt_wager, 0);
        assert_eq!(book_state.bet_type, bet_type);
        assert_eq!(book_state.total_dispute_stake, 0);
        assert!(book_state.dispute_resolution_result.is_none());
        assert!(book_state.concluded_at.is_none());
        assert!(book_state.oracles.is_empty());
        assert!(book_state.bets_for.is_empty());
        assert!(book_state.bets_against.is_empty());
        assert!(book_state.positions.is_empty());
        // the book ata account should be created
        let book_ata_account_state: anchor_spl::token::spl_token::state::Account =
            banks_client.get_packed_account_data(book_ata).await.unwrap();
        assert_eq!(book_ata_account_state.owner, book_pda);
        assert_eq!(book_ata_account_state.mint, USDC);
        assert_eq!(book_ata_account_state.amount, 0);
    }
}
