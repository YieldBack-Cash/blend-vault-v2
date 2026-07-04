use soroban_sdk::contracterror;

/// The error codes for the contract.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum BlendVaultError {
    // Default errors to align with built-in contract
    BalanceError = 10,

    ReserveNotFound = 100,
    ReserveAlreadyExists = 101,
    InvalidAmount = 102,
    InsufficientReserves = 105,
    InvalidBTokensMinted = 106,
    InvalidBTokensBurnt = 107,
    InvalidSharesMinted = 108,
    InvalidSharesBurnt = 112,
    SwapNotConfigured = 113,
}