use anchor_lang::prelude::*;

use crate::state::user_account::UserAccount;

#[derive(Accounts)]
pub struct UserAccountInitAccounts<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(init,payer=user,space=UserAccount::INIT_SPACE,seeds=[b"UserAccount".as_ref(),user.key().as_ref()],bump)]
    pub user_account_pda: Account<'info, UserAccount>,
    pub system_program: Program<'info, System>,
}

pub fn user_account_init(ctx: Context<UserAccountInitAccounts>) -> Result<()> {
    ctx.accounts.user_account_pda.authority = ctx.accounts.user.key();
    Ok(())
}

#[cfg(test)]
mod test {
    use std::rc::Rc;

    use anchor_client::RequestBuilder;
    use anchor_lang::AccountDeserialize;
    use solana_program_test::{tokio, ProgramTest};
    use solana_sdk::{
        account::Account, native_token::LAMPORTS_PER_SOL, pubkey::Pubkey, rent::Rent,
        signature::Keypair, signer::Signer, system_program, transaction::Transaction,
    };

    use crate::state::UserAccount;

    #[tokio::test]
    async fn test_user_account_init_success() {
        let program_id = crate::id();
        let mut program_test = ProgramTest::new("betting", program_id, None);

        let user = Keypair::new();
        program_test.add_account(
            user.pubkey(),
            Account {
                lamports: LAMPORTS_PER_SOL,
                ..Default::default()
            },
        );

        let (user_pda, _) = Pubkey::find_program_address(
            &[b"UserAccount".as_ref(), user.pubkey().as_ref()],
            &program_id,
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
            .signer(&user)
            .accounts(crate::accounts::UserAccountInitAccounts {
                user: user.pubkey(),
                user_account_pda: user_pda,
                system_program: system_program::ID,
            })
            .args(crate::instruction::UserAccountInit)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &user],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the user should pay lamports for rent
        let user_system_account = banks_client
            .get_account(user.pubkey())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            user_system_account.lamports,
            LAMPORTS_PER_SOL - Rent::default().minimum_balance(UserAccount::INIT_SPACE)
        );
        // the user account pda should be created
        let user_pda_account = banks_client.get_account(user_pda).await.unwrap().unwrap();
        let user_pda_account_state =
            UserAccount::try_deserialize(&mut user_pda_account.data.as_slice()).unwrap();
        assert_eq!(user_pda_account_state.authority, user.pubkey());
        assert!(user_pda_account_state.books_bet_on.is_empty());
        assert!(user_pda_account_state.books_oracled.is_empty());
        assert_eq!(user_pda_account_state.books_initialized, 0);
    }
}
