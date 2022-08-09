pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;
use instructions::*;

declare_id!("AdffT7aGQ2gnavJutMWpHWph8kXnir1kCZs99YPTue87");

#[program]
pub mod betting {
    use super::*;

    pub fn user_account_init(ctx: Context<UserAccountInitAccounts>) -> Result<()> {
        instructions::user_account_init(ctx)
    }
    pub fn user_account_close(ctx: Context<UserAccountCloseAccounts>) -> Result<()> {
        instructions::user_account_close(ctx)
    }
    pub fn user_account_shrink(ctx: Context<UserAccountShrinkAccounts>) -> Result<()> {
        instructions::user_account_shrink(ctx)
    }
    pub fn game_init(
        ctx: Context<GameInitAccounts>,
        game_id: u32,
        league_id: u32,
        home_team_id: u32,
        away_team_id: u32,
        kickoff: i64,
    ) -> Result<()> {
        instructions::game_init(ctx, game_id, league_id, home_team_id, away_team_id, kickoff)
    }
}
