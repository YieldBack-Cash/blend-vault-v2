use soroban_sdk::Env;

use crate::errors::BlendVaultError;

/// Panics with `err` if `amount` is not positive.
pub fn require_positive(e: &Env, amount: i128, err: BlendVaultError) {
    if amount <= 0 {
        soroban_sdk::panic_with_error!(e, err);
    }
}