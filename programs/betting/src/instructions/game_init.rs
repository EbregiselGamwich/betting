use anchor_lang::prelude::*;

use crate::{constants::OPERATOR_PUBKEY, state::Game};

#[derive(Accounts)]
#[instruction(game_id:u32)]
pub struct GameInitAccounts<'info> {
    #[account(mut,address=OPERATOR_PUBKEY)]
    pub operator: Signer<'info>,
    #[account(init,payer=operator,space=Game::INIT_SPACE,seeds=[b"Game".as_ref(),&game_id.to_le_bytes()],bump)]
    pub game_pda: Account<'info, Game>,
    pub system_program: Program<'info, System>,
}

pub fn game_init(
    ctx: Context<GameInitAccounts>,
    game_id: u32,
    league_id: u32,
    home_team_id: u32,
    away_team_id: u32,
    kickoff: i64,
) -> Result<()> {
    ctx.accounts.game_pda.set_inner(Game {
        game_id,
        league_id,
        home_team_id,
        away_team_id,
        kickoff,
        books_count: 0,
    });
    Ok(())
}

#[cfg(test)]
mod test {
    use std::rc::Rc;

    use anchor_client::RequestBuilder;
    use anchor_lang::AccountDeserialize;
    use home::home_dir;
    use solana_program_test::{tokio, ProgramTest};
    use solana_sdk::{
        account::Account,
        native_token::LAMPORTS_PER_SOL,
        pubkey::Pubkey,
        signature::{read_keypair_file, Keypair},
        signer::Signer,
        system_program,
        transaction::Transaction,
    };

    use crate::state::Game;

    #[tokio::test]
    async fn test_game_init_success() {
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
            .accounts(crate::accounts::GameInitAccounts {
                operator: operator.pubkey(),
                game_pda,
                system_program: system_program::id(),
            })
            .args(crate::instruction::GameInit {
                game_id,
                league_id,
                home_team_id,
                away_team_id,
                kickoff,
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &operator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the game pda account should be created
        let game_account = banks_client.get_account(game_pda).await.unwrap().unwrap();
        let game_state = Game::try_deserialize(&mut game_account.data.as_slice()).unwrap();
        assert_eq!(game_state.game_id, 1);
        assert_eq!(game_state.league_id, 1);
        assert_eq!(game_state.home_team_id, 1);
        assert_eq!(game_state.away_team_id, 1);
        assert_eq!(game_state.kickoff, 1);
        assert_eq!(game_state.books_count, 0);
    }

    #[tokio::test]
    #[should_panic(expected = "Custom(2012)")] // ConstraintAddress
    async fn test_game_init_err_wrong_operator() {
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
            .accounts(crate::accounts::GameInitAccounts {
                operator: operator.pubkey(),
                game_pda,
                system_program: system_program::id(),
            })
            .args(crate::instruction::GameInit {
                game_id,
                league_id,
                home_team_id,
                away_team_id,
                kickoff,
            })
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &operator],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the game pda account should be created
        let game_account = banks_client.get_account(game_pda).await.unwrap().unwrap();
        let game_state = Game::try_deserialize(&mut game_account.data.as_slice()).unwrap();
        assert_eq!(game_state.game_id, 1);
        assert_eq!(game_state.league_id, 1);
        assert_eq!(game_state.home_team_id, 1);
        assert_eq!(game_state.away_team_id, 1);
        assert_eq!(game_state.kickoff, 1);
        assert_eq!(game_state.books_count, 0);
    }
}
