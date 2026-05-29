use soroban_sdk::Env;

use crate::errors::FeeVaultError;

/// Require that an incoming amount is positive
///
/// ### Panics
/// If the number is negative or zero
pub fn require_positive(e: &Env, amount: i128, err: FeeVaultError) {
    if amount <= 0 {
        soroban_sdk::panic_with_error!(e, err);
    }
}