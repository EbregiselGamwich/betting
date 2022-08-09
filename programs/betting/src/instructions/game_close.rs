use anchor_lang::prelude::*;

use crate::{constants::OPERATOR_PUBKEY, error::BettingError, state::Game};

#[derive(Accounts)]
pub struct GameCloseAccounts<'info> {
    #[account(mut,address=OPERATOR_PUBKEY)]
    pub operator: Signer<'info>,
    #[account(mut,close=operator,seeds=[b"Game".as_ref(),&game_pda.game_id.to_le_bytes()],bump)]
    pub game_pda: Account<'info, Game>,
}

pub fn game_close(ctx: Context<GameCloseAccounts>) -> Result<()> {
    require!(
        ctx.accounts.game_pda.books_count == 0,
        BettingError::UnsettledBooksRemaining
    );
    Ok(())
}

#[cfg(test)]
mod test {
    use std::rc::Rc;

    use anchor_client::RequestBuilder;
    use anchor_lang::AccountSerialize;
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

    use crate::state::Game;

    #[tokio::test]
    async fn test_game_close_success() {
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

        let game_id: u32 = 1;
        let league_id: u32 = 1;
        let home_team_id: u32 = 1;
        let away_team_id: u32 = 1;
        let kickoff: i64 = 1;
        let (game_pda, _) =
            Pubkey::find_program_address(&[b"Game".as_ref(), &game_id.to_le_bytes()], &program_id);
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
            .accounts(crate::accounts::GameCloseAccounts {
                operator: operator.pubkey(),
                game_pda,
            })
            .args(crate::instruction::GameClose)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &operator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the game pda account should be closed
        assert!(banks_client.get_account(game_pda).await.unwrap().is_none());
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(2012)")] // ConstraintAddress
    async fn test_game_close_err_wrong_operator() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let operator = Keypair::new();
        program_test.add_account(
            operator.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let game_id: u32 = 1;
        let league_id: u32 = 1;
        let home_team_id: u32 = 1;
        let away_team_id: u32 = 1;
        let kickoff: i64 = 1;
        let (game_pda, _) =
            Pubkey::find_program_address(&[b"Game".as_ref(), &game_id.to_le_bytes()], &program_id);
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
            .accounts(crate::accounts::GameCloseAccounts {
                operator: operator.pubkey(),
                game_pda,
            })
            .args(crate::instruction::GameClose)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &operator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the game pda account should be closed
        assert!(banks_client.get_account(game_pda).await.unwrap().is_none());
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(6001)")] // unsettled books
    async fn test_game_close_err_unsettled_books() {
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

        let game_id: u32 = 1;
        let league_id: u32 = 1;
        let home_team_id: u32 = 1;
        let away_team_id: u32 = 1;
        let kickoff: i64 = 1;
        let (game_pda, _) =
            Pubkey::find_program_address(&[b"Game".as_ref(), &game_id.to_le_bytes()], &program_id);
        let game_pda_state = Game {
            game_id,
            league_id,
            home_team_id,
            away_team_id,
            kickoff,
            books_count: 2,
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
            .accounts(crate::accounts::GameCloseAccounts {
                operator: operator.pubkey(),
                game_pda,
            })
            .args(crate::instruction::GameClose)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &operator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the game pda account should be closed
        assert!(banks_client.get_account(game_pda).await.unwrap().is_none());
    }
}
