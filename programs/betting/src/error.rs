use anchor_lang::prelude::*;

#[error_code]
pub enum BettingError {
    #[msg("NoAuthority")]
    NoAuthority = 0,
    #[msg("UnsettledBooksRemaining")]
    UnsettledBooksRemaining = 1,
    #[msg("BookNotSettled")]
    BookNotSettled = 2,
    #[msg("UserAlreadyOptIn")]
    UserAlreadyOptIn = 3,
    #[msg("UserDidNotOptIn")]
    UserDidNotOptIn = 4,
    #[msg("MinTokenAmountNotMet")]
    MinTokenAmountNotMet = 5,
    #[msg("NotInWindow")]
    NotInWindow = 6,
    #[msg("NotFound")]
    NotFound = 7,
    #[msg("NoResultYet")]
    NoResultYet = 8,
}
