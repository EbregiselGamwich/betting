use anchor_lang::prelude::*;

#[error_code]
pub enum BettingError {
    #[msg("NoAuthority")]
    NoAuthority = 0,
}
