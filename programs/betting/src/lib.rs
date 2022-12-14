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
    pub fn book_bettor_opt_int(ctx: Context<BookBettorOptInAccounts>) -> Result<()> {
        instructions::book_bettor_opt_int(ctx)
    }
    pub fn book_bettor_place_bet(
        ctx: Context<BookBettorPlaceBetAccounts>,
        odds: u32,
        wager: u64,
        bet_direction: BetDirection,
    ) -> Result<()> {
        instructions::book_bettor_place_bet(ctx, odds, wager, bet_direction)
    }
    pub fn book_bettor_cancel_bet(
        ctx: Context<BookBettorCancelBetAccounts>,
        bet_id: u64,
        bet_direction: BetDirection,
    ) -> Result<()> {
        instructions::book_bettor_cancel_bet(ctx, bet_id, bet_direction)
    }
    pub fn book_match_bets(ctx: Context<BookMatchBetsAccounts>) -> Result<()> {
        instructions::book_match_bets(ctx)
    }
    pub fn book_oracle_opt_in(ctx: Context<BookOracleOptInAccounts>, stake: u64) -> Result<()> {
        instructions::book_oracle_opt_in(ctx, stake)
    }
    pub fn book_oracle_add_stake(ctx: Context<BookOracleAddStakeAccounts>, stake: u64) -> Result<()> {
        instructions::book_oracle_add_stake(ctx, stake)
    }
    pub fn book_oracle_update_outcome(
        ctx: Context<BookOracleUpdateOutcomeAccounts>,
        bet_outcome: Option<BetOutcome>,
    ) -> Result<()> {
        instructions::book_oracle_update_outcome(ctx, bet_outcome)
    }
    pub fn book_bettor_dispute(ctx: Context<BookBettorDisputeAccounts>, stake: u64) -> Result<()> {
        instructions::book_bettor_dispute(ctx, stake)
    }
    pub fn book_bettor_cancel_dispute(ctx: Context<BookBettorCancelDisputeAccounts>) -> Result<()> {
        instructions::book_bettor_cancel_dispute(ctx)
    }
    pub fn book_operator_resolve_dispute(
        ctx: Context<BookOperatorResolveDisputeAccounts>,
        bet_outcome: BetOutcome,
    ) -> Result<()> {
        instructions::book_operator_resolve_dispute(ctx, bet_outcome)
    }
    pub fn book_bettor_settle(ctx: Context<BookBettorSettleAccounts>) -> Result<()> {
        instructions::book_bettor_settle(ctx)
    }
    pub fn book_oracle_settle(ctx: Context<BookOracleSettleAccounts>) -> Result<()> {
        instructions::book_oracle_settle(ctx)
    }
    pub fn book_initiator_settle(ctx: Context<BookInitiatorSettleAccounts>) -> Result<()> {
        instructions::book_initiator_settle(ctx)
    }
}
