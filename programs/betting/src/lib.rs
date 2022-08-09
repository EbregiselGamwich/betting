pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;
use instructions::*;
use state::*;

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
    pub fn game_close(ctx: Context<GameCloseAccounts>) -> Result<()> {
        instructions::game_close(ctx)
    }
    pub fn book_init(ctx: Context<BookInitAccounts>, bet_type: BetType) -> Result<()> {
        instructions::book_init(ctx, bet_type)
    }
    pub fn book_close(ctx: Context<BookCloseAccounts>) -> Result<()> {
        instructions::book_close(ctx)
    }
}
