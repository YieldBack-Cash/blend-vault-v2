use soroban_sdk::{contractclient, token::TokenClient, vec, Address, Env, Vec};

#[contractclient(name = "SoroswapRouterClient")]
pub trait SoroswapRouter {
    fn swap_exact_tokens_for_tokens(
        e: Env,
        amount_in: i128,
        amount_out_min: i128,
        path: Vec<Address>,
        to: Address,
        deadline: u64,
    ) -> Vec<i128>;
}

/// Swaps `amount_in` of `blnd` for `asset` via the Soroswap router.
/// Returns the amount of `asset` received.
pub fn swap_blnd_for_asset(
    e: &Env,
    router: &Address,
    blnd: &Address,
    asset: &Address,
    amount_in: i128,
    amount_out_min: i128,
) -> i128 {
    TokenClient::new(e, blnd).approve(
        &e.current_contract_address(),
        router,
        &amount_in,
        &(e.ledger().sequence() + 1),
    );

    let path = vec![e, blnd.clone(), asset.clone()];
    let deadline = e.ledger().timestamp() + 300;

    let amounts = SoroswapRouterClient::new(e, router).swap_exact_tokens_for_tokens(
        &amount_in,
        &amount_out_min,
        &path,
        &e.current_contract_address(),
        &deadline,
    );

    amounts.get(1).unwrap()
}