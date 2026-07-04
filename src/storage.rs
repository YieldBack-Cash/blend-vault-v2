use soroban_sdk::{contracttype, panic_with_error, unwrap::UnwrapOptimized, Address, Env, Symbol};

use crate::{errors::BlendVaultError, vault::VaultData};

//---------- Storage Keys ----------//

const POOL_KEY: &str = "Pool";
const ADMIN_KEY: &str = "Admin";
const ASSET_KEY: &str = "Asset";
const SIGNER_KEY: &str = "Signer";
const VAULT_DATA_KEY: &str = "Vault";
const ROUTER_KEY: &str = "Router";
const BLND_TOKEN_KEY: &str = "BlndToken";

#[derive(Clone)]
#[contracttype]
pub enum BlendVaultDataKey {
    Shares(Address),
    Allowance(AllowanceKey),
}

#[derive(Clone)]
#[contracttype]
pub struct AllowanceKey {
    pub from: Address,
    pub spender: Address,
}

#[derive(Clone)]
#[contracttype]
pub struct AllowanceValue {
    pub amount: i128,
    pub expiration_ledger: u32,
}

//---------- Storage Utils ----------//

pub const ONE_DAY_LEDGERS: u32 = 17280; // assumes 5 seconds per ledger on average

const LEDGER_BUMP_SHARED: u32 = 31 * ONE_DAY_LEDGERS;
const LEDGER_THRESHOLD_SHARED: u32 = LEDGER_BUMP_SHARED - ONE_DAY_LEDGERS;

const LEDGER_BUMP_USER: u32 = 120 * ONE_DAY_LEDGERS;
const LEDGER_THRESHOLD_USER: u32 = LEDGER_BUMP_USER - 20 * ONE_DAY_LEDGERS;

/// Extends the instance storage TTL.
pub fn extend_instance(e: &Env) {
    e.storage()
        .instance()
        .extend_ttl(LEDGER_THRESHOLD_SHARED, LEDGER_BUMP_SHARED);
}

//---------- Instance ----------//

/// Get the pool address
pub fn get_pool(e: &Env) -> Address {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, POOL_KEY))
        .unwrap_optimized()
}

/// Set the pool address
pub fn set_pool(e: &Env, pool: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, POOL_KEY), &pool);
}

/// Get the admin address
pub fn get_admin(e: &Env) -> Address {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, ADMIN_KEY))
        .unwrap_optimized()
}

/// Set the admin address
pub fn set_admin(e: &Env, admin: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, ADMIN_KEY), &admin);
}

/// Get the asset address
pub fn get_asset(e: &Env) -> Address {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, ASSET_KEY))
        .unwrap_optimized()
}

/// Set the asset address
pub fn set_asset(e: &Env, asset: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, ASSET_KEY), &asset);
}

/// Get the signer address. Can be None if no signer is set.
pub fn get_signer(e: &Env) -> Option<Address> {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, SIGNER_KEY))
}

/// Set the signer address.
pub fn set_signer(e: &Env, signer: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, SIGNER_KEY), &signer);
}

/// Delete the signer address.
pub fn del_signer(e: &Env) {
    e.storage()
        .instance()
        .remove::<Symbol>(&Symbol::new(e, SIGNER_KEY));
}

/// Get the Soroswap router address. Returns None if not configured.
pub fn get_router(e: &Env) -> Option<Address> {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, ROUTER_KEY))
}

/// Set the Soroswap router address.
pub fn set_router(e: &Env, router: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, ROUTER_KEY), &router);
}

/// Get the BLND token address.
pub fn get_blnd_token(e: &Env) -> Address {
    e.storage()
        .instance()
        .get::<Symbol, Address>(&Symbol::new(e, BLND_TOKEN_KEY))
        .unwrap_optimized()
}

/// Set the BLND token address.
pub fn set_blnd_token(e: &Env, blnd_token: Address) {
    e.storage()
        .instance()
        .set::<Symbol, Address>(&Symbol::new(e, BLND_TOKEN_KEY), &blnd_token);
}

//---------- Persistent ----------//
// Persistent data is not bumped on read — entries are almost always written when accessed,
// so bumping on write is sufficient. Off-chain reads (e.g. dApp) don't need to extend TTL.

/// Persists vault data to storage.
pub fn set_vault_data(e: &Env, vault: &VaultData) {
    let key = Symbol::new(e, VAULT_DATA_KEY);
    e.storage()
        .persistent()
        .set::<Symbol, VaultData>(&key, vault);
    e.storage()
        .persistent()
        .extend_ttl(&key, LEDGER_THRESHOLD_USER, LEDGER_BUMP_USER);
}

/// Get the vault data
pub fn get_vault_data(e: &Env) -> VaultData {
    let key = Symbol::new(e, VAULT_DATA_KEY);
    e.storage()
        .persistent()
        .get::<Symbol, VaultData>(&key)
        .unwrap_or_else(|| panic_with_error!(e, BlendVaultError::ReserveNotFound))
}

/// Persists `user`'s vault share balance. Values use 7 decimal places of precision.
pub fn set_vault_shares(e: &Env, user: &Address, shares: i128) {
    let key = BlendVaultDataKey::Shares(user.clone());
    e.storage()
        .persistent()
        .set::<BlendVaultDataKey, i128>(&key, &shares);
    e.storage()
        .persistent()
        .extend_ttl(&key, LEDGER_THRESHOLD_USER, LEDGER_BUMP_USER);
}

/// Returns `user`'s vault share balance. Values use 7 decimal places of precision.
pub fn get_vault_shares(e: &Env, user: &Address) -> i128 {
    let key = BlendVaultDataKey::Shares(user.clone());
    e.storage()
        .persistent()
        .get::<BlendVaultDataKey, i128>(&key)
        .unwrap_or(0)
}

//---------- Temporary ----------//

/// Get the approved allowance for a spender on behalf of from.
/// Returns 0 if no allowance exists or if it has expired.
pub fn get_allowance(e: &Env, from: &Address, spender: &Address) -> i128 {
    get_allowance_with_expiration(e, from, spender).0
}

/// Get the approved allowance and its expiration ledger together.
/// Returns (0, 0) if no allowance exists or if it has expired.
pub fn get_allowance_with_expiration(e: &Env, from: &Address, spender: &Address) -> (i128, u32) {
    let key = BlendVaultDataKey::Allowance(AllowanceKey {
        from: from.clone(),
        spender: spender.clone(),
    });
    match e.storage().temporary().get::<BlendVaultDataKey, AllowanceValue>(&key) {
        Some(a) if a.expiration_ledger >= e.ledger().sequence() => (a.amount, a.expiration_ledger),
        _ => (0, 0),
    }
}

/// Set an allowance expiring at `expiration_ledger`.
/// The temporary storage TTL is extended to match the expiration.
pub fn set_allowance(
    e: &Env,
    from: &Address,
    spender: &Address,
    amount: i128,
    expiration_ledger: u32,
) {
    let key = BlendVaultDataKey::Allowance(AllowanceKey {
        from: from.clone(),
        spender: spender.clone(),
    });
    e.storage().temporary().set::<BlendVaultDataKey, AllowanceValue>(
        &key,
        &AllowanceValue { amount, expiration_ledger },
    );
    let current = e.ledger().sequence();
    if expiration_ledger > current {
        let ttl = expiration_ledger - current;
        e.storage().temporary().extend_ttl(&key, ttl, ttl);
    }
}
