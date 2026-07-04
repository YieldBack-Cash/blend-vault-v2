use blend_contract_sdk::pool::{Client as PoolClient, Request};
use soroban_sdk::{vec, Address, Env, Vec};

/// Executes a supply of a specific reserve into the underlying pool on behalf of the blend vault
///
/// ### Arguments
/// * `pool` - The pool address
/// * `reserve` - The reserve address
/// * `from` - The address of the user
/// * `amount` - The amount of tokens to deposit
pub fn supply(e: &Env, pool: &Address, reserve: &Address, from: &Address, amount: i128) {
    PoolClient::new(&e, &pool).submit(
        &e.current_contract_address(),
        &from,
        &from,
        &vec![
            &e,
            Request {
                address: reserve.clone(),
                amount,
                request_type: 0,
            },
        ],
    );
}

/// Executes a user withdrawal of a specific reserve from the underlying pool on behalf of the blend vault
///
/// ### Arguments
/// * `pool` - The pool address
/// * `reserve` - The reserve address
/// * `to` - The destination of the withdrawal
/// * `amount` - The amount of tokens to withdraw
pub fn withdraw(e: &Env, pool: &Address, reserve: &Address, to: &Address, amount: i128) {
    PoolClient::new(&e, &pool).submit(
        &e.current_contract_address(),
        &e.current_contract_address(),
        &to,
        &vec![
            &e,
            Request {
                address: reserve.clone(),
                amount,
                request_type: 1,
            },
        ],
    );
}

/// Executes a claim of BLND emissions from the pool on behalf of the blend vault
///
/// ### Arguments
/// * `pool` - The pool address
/// * `reserve_token_ids` - The reserve token IDs to claim emissions for
/// * `to` - The address to send the emissions to
///
/// ### Returns
/// * `i128` - The amount of emissions claimed
pub fn claim(e: &Env, pool: &Address, reserve_token_ids: &Vec<u32>, to: &Address) -> i128 {
    PoolClient::new(&e, &pool).claim(&e.current_contract_address(), reserve_token_ids, to)
}

/// Fetches the reserve's b_rate from the pool
///
/// ### Arguments
/// * `pool` - The pool address
/// * `reserve` - The reserve address to fetch the b_rate for
///
/// ### Returns
/// * `i128` - The b_rate of the reserve
pub fn reserve_b_rate(e: &Env, pool: &Address, reserve: &Address) -> i128 {
    PoolClient::new(&e, &pool).get_reserve(reserve).data.b_rate
}

/// Returns the emission token ID for the reserve's supply (bToken) position.
/// Token ID = reserve_index * 2 + 1
pub fn reserve_supply_token_id(e: &Env, pool: &Address, reserve: &Address) -> u32 {
    PoolClient::new(e, pool).get_reserve(reserve).config.index * 2 + 1
}

/// Returns the vault's actual bToken balance for a reserve by reading the pool's positions.
pub fn vault_b_token_balance(e: &Env, pool: &Address, reserve: &Address, vault: &Address) -> i128 {
    let reserve_index = PoolClient::new(e, pool).get_reserve(reserve).config.index;
    PoolClient::new(e, pool)
        .get_positions(vault)
        .supply
        .get(reserve_index)
        .unwrap_or(0)
}
