use anchor_lang::prelude::*;

use crate::state::UserAccount;

#[derive(Accounts)]
pub struct UserAccountShrinkAccounts<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    #[account(mut,realloc=user_account_pda.current_space(),realloc::payer=user,realloc::zero=false,seeds=[b"UserAccount".as_ref(),user.key().as_ref()],bump)]
    pub user_account_pda: Account<'info, UserAccount>,
    pub system_program: Program<'info, System>,
}

pub fn user_account_shrink(_ctx: Context<UserAccountShrinkAccounts>) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod test {
    use std::{
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
    };

    use anchor_client::RequestBuilder;
    use anchor_lang::AccountSerialize;
    use solana_program_test::{tokio, ProgramTest};
    use solana_sdk::{
        account::Account, native_token::LAMPORTS_PER_SOL, pubkey::Pubkey, rent::Rent,
        signature::Keypair, signer::Signer, system_program, transaction::Transaction,
    };

    use crate::state::UserAccount;

    #[tokio::test]
    async fn test_user_account_shrink_success() {
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
        let user_pda_state = UserAccount {
            authority: user.pubkey(),
            books_initialized: 0,
            books_oracled: BTreeSet::new(),
            bets: BTreeMap::new(),
        };
        let mut user_pda_data: Vec<u8> = Vec::new();
        user_pda_state.try_serialize(&mut user_pda_data).unwrap();
        user_pda_data.resize(user_pda_state.current_space() * 2, 0);
        program_test.add_account(
            user_pda,
            Account {
                lamports: LAMPORTS_PER_SOL,
                data: user_pda_data,
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
            .signer(&user)
            .accounts(crate::accounts::UserAccountShrinkAccounts {
                user: user.pubkey(),
                user_account_pda: user_pda,
                system_program: system_program::id(),
            })
            .args(crate::instruction::UserAccountShrink)
            .instructions()
            .unwrap();
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer, &user],
            recent_blockhash,
        );
        banks_client.process_transaction(tx).await.unwrap();

        // the unnesseccery lamports should be return to the user system account
        let user_system_account = banks_client
            .get_account(user.pubkey())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            user_system_account.lamports,
            LAMPORTS_PER_SOL
                + (LAMPORTS_PER_SOL
                    - Rent::default().minimum_balance(user_pda_state.current_space()))
        );
        // the user account pda should be at min space and min rent
        let user_account_pda_account = banks_client.get_account(user_pda).await.unwrap().unwrap();
        assert_eq!(
            user_account_pda_account.data.len(),
            user_pda_state.current_space()
        );
        assert_eq!(
            user_account_pda_account.lamports,
            Rent::default().minimum_balance(user_pda_state.current_space())
        );
    }
}
