use anchor_lang::prelude::*;

#[account]
pub struct Game {
    pub game_id: u32,
    pub league_id: u32,
    pub home_team_id: u32,
    pub away_team_id: u32,
    pub kickoff: i64,
    pub books_count: u32,
}
impl Game {
    pub const INIT_SPACE: usize = 8 + 4 + 4 + 4 + 4 + 8 + 4;
}
