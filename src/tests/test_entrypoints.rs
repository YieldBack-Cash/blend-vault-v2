#![cfg(test)]

use crate::{
    constants::SCALAR_12,
    storage,
    testutils::{assert_approx_eq_rel, mockpool, register_blend_vault, EnvTestUtils},
    vault::VaultData,
    BlendVaultClient,
};
use soroban_sdk::{
    testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation},
    vec, Address, Env, IntoVal, Symbol,
};

const INIT_B_RATE: i128 = 1_000_000_000_000;

/// Registers a vault administered by `admin`, backed by a mock pool at
/// `INIT_B_RATE`. Returns (vault address, vault client, mock pool client).
fn setup<'a>(
    e: &'a Env,
    admin: &Address,
) -> (Address, BlendVaultClient<'a>, mockpool::MockPoolClient<'a>) {
    let pool_client = mockpool::register_mock_pool_with_b_rate(e, INIT_B_RATE);
    let reserve = Address::generate(e);
    let blnd_token = Address::generate(e);
    let vault_address = register_blend_vault(e, admin, &pool_client.address, &reserve, &blnd_token);
    let vault_client = BlendVaultClient::new(e, &vault_address);
    (vault_address, vault_client, pool_client)
}

/// Writes vault state of 1000 bTokens / 1200 shares, split 10% to samwise
/// and 90% to frodo.
fn seed_vault_positions(e: &Env, vault_address: &Address, samwise: &Address, frodo: &Address) {
    e.as_contract(vault_address, || {
        let vault_data = VaultData {
            total_b_tokens: 1000_0000000,
            total_shares: 1200_0000000,
            b_rate: INIT_B_RATE,
            last_update_timestamp: e.ledger().timestamp(),
        };
        storage::set_vault_data(e, &vault_data);
        storage::set_vault_shares(e, samwise, 120_0000000);
        storage::set_vault_shares(e, frodo, 1080_0000000);
    });
}

#[test]
fn test_constructor_ok() {
    let e = Env::default();
    e.mock_all_auths();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    // registered inline (not via `setup`) so the constructor args can be
    // asserted against the recorded authorization below
    let pool = mockpool::register_mock_pool_with_b_rate(&e, INIT_B_RATE).address;
    let reserve = Address::generate(&e);
    let blnd_token = Address::generate(&e);
    let vault_address = register_blend_vault(&e, &samwise, &pool, &reserve, &blnd_token);

    assert_eq!(
        e.auths()[0],
        (
            samwise.clone(),
            AuthorizedInvocation {
                function: AuthorizedFunction::Contract((
                    vault_address.clone(),
                    Symbol::new(&e, "__constructor"),
                    vec![
                        &e,
                        samwise.into_val(&e),
                        pool.into_val(&e),
                        reserve.into_val(&e),
                        blnd_token.into_val(&e),
                    ]
                )),
                sub_invocations: std::vec![]
            }
        )
    );

    let client = BlendVaultClient::new(&e, &vault_address);
    client.set_signer(&Some(frodo.clone()));

    assert_eq!(client.get_config(), (pool.clone(), reserve.clone()));
    assert_eq!(client.query_asset(), reserve);
    assert_eq!(client.get_admin(), samwise);
    assert_eq!(client.get_signer(), Some(frodo));
    let vault_data = client.get_vault();
    assert_eq!(vault_data.total_b_tokens, 0);
    assert_eq!(vault_data.total_shares, 0);
    assert_eq!(vault_data.b_rate, INIT_B_RATE);
    assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
}

#[test]
fn test_get_b_tokens() {
    let e = Env::default();
    e.mock_all_auths();
    e.set_default_info();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let (vault_address, vault_client, mock_client) = setup(&e, &samwise);
    seed_vault_positions(&e, &vault_address, &samwise, &frodo);

    assert_eq!(vault_client.get_b_tokens(&samwise), 100_0000000);
    assert_eq!(vault_client.get_b_tokens(&frodo), 900_0000000);

    // b_rate increases by 10%; without fees all b_tokens stay with depositors
    mock_client.set_b_rate(&1_100_000_000_000);
    e.jump(5);

    assert_eq!(vault_client.get_b_tokens(&samwise), 100_0000000);
    assert_eq!(vault_client.get_b_tokens(&frodo), 900_0000000);

    // The view function shouldn't mutate the state
    e.as_contract(&vault_address, || {
        let reserve_vault = storage::get_vault_data(&e);
        assert_eq!(reserve_vault.total_b_tokens, 1000_0000000);
        assert_eq!(reserve_vault.total_shares, 1200_0000000);
        assert_eq!(reserve_vault.b_rate, INIT_B_RATE);
    });

    // Should return 0 if user doesn't have any shares
    let non_existent_user = Address::generate(&e);
    assert_eq!(vault_client.get_b_tokens(&non_existent_user), 0);
}

#[test]
fn test_underlying_wrappers() {
    let e = Env::default();
    e.mock_all_auths();
    e.set_default_info();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let (vault_address, vault_client, mock_client) = setup(&e, &samwise);
    seed_vault_positions(&e, &vault_address, &samwise, &frodo);

    let total_underlying_value = INIT_B_RATE * 1000_0000000 / SCALAR_12;
    let frodo_underlying = vault_client.get_underlying_tokens(&frodo);
    let samwise_underlying = vault_client.get_underlying_tokens(&samwise);

    assert_eq!(
        frodo_underlying + samwise_underlying,
        total_underlying_value
    );
    assert_eq!(frodo_underlying, 9 * samwise_underlying);

    // b_rate increases by 10%; all yield goes to depositors with no fee
    mock_client.set_b_rate(&1_100_000_000_000);
    e.jump(5);

    let sam_underlying_after = vault_client.get_underlying_tokens(&samwise);
    let frodo_underlying_after = vault_client.get_underlying_tokens(&frodo);

    // Each depositor earns the full 10% gain
    assert_approx_eq_rel(
        frodo_underlying_after + sam_underlying_after,
        110 * total_underlying_value / 100,
        0_0000001,
    );
    assert_eq!(frodo_underlying_after, 110 * frodo_underlying / 100);
    assert_eq!(sam_underlying_after, 110 * samwise_underlying / 100);
    assert_eq!(frodo_underlying_after, 9 * sam_underlying_after);

    // Ensure the view function never panics
    let non_existent_user = Address::generate(&e);
    assert_eq!(vault_client.get_underlying_tokens(&non_existent_user), 0);
}

#[test]
fn test_set_admin() {
    let e = Env::default();
    e.mock_all_auths();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);

    let (vault_address, vault_client, _mock_client) = setup(&e, &samwise);

    e.as_contract(&vault_address, || {
        assert_eq!(storage::get_admin(&e), samwise.clone());
    });

    vault_client.set_admin(&frodo);

    let authorized_function = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            vault_address.clone(),
            Symbol::new(&e, "set_admin"),
            vec![&e, frodo.into_val(&e)],
        )),
        sub_invocations: std::vec![],
    };
    assert_eq!(
        e.auths(),
        std::vec![
            (samwise.clone(), authorized_function.clone()),
            (frodo.clone(), authorized_function)
        ]
    );

    e.as_contract(&vault_address, || {
        assert_eq!(storage::get_admin(&e), frodo);
    });

    let new_admin = Address::generate(&e);
    vault_client.set_admin(&new_admin);

    let new_authorized_function = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            vault_address.clone(),
            Symbol::new(&e, "set_admin"),
            vec![&e, new_admin.into_val(&e)],
        )),
        sub_invocations: std::vec![],
    };
    assert_eq!(
        e.auths(),
        std::vec![
            (frodo.clone(), new_authorized_function.clone()),
            (new_admin.clone(), new_authorized_function)
        ]
    );
}

#[test]
fn test_set_signer() {
    let e = Env::default();
    e.mock_all_auths();

    let samwise = Address::generate(&e);
    let frodo = Address::generate(&e);
    let merry = Address::generate(&e);

    let (vault_address, vault_client, _mock_client) = setup(&e, &samwise);
    vault_client.set_signer(&Some(merry.clone()));

    e.as_contract(&vault_address, || {
        assert_eq!(storage::get_signer(&e), Some(merry.clone()));
    });

    vault_client.set_signer(&Some(frodo.clone()));

    let authorized_function = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            vault_address.clone(),
            Symbol::new(&e, "set_signer"),
            vec![&e, Some(frodo.clone()).into_val(&e)],
        )),
        sub_invocations: std::vec![],
    };
    assert_eq!(
        e.auths(),
        std::vec![
            (samwise.clone(), authorized_function.clone()),
            (frodo.clone(), authorized_function)
        ]
    );

    e.as_contract(&vault_address, || {
        assert_eq!(storage::get_signer(&e), Some(frodo.clone()));
    });

    // validate signer removal
    vault_client.set_signer(&None);
    let authorized_function = AuthorizedInvocation {
        function: AuthorizedFunction::Contract((
            vault_address.clone(),
            Symbol::new(&e, "set_signer"),
            vec![&e, None::<Address>.into_val(&e)],
        )),
        sub_invocations: std::vec![],
    };
    assert_eq!(
        e.auths(),
        std::vec![(samwise.clone(), authorized_function.clone()),]
    );

    e.as_contract(&vault_address, || {
        assert_eq!(storage::get_signer(&e), None);
    });
}