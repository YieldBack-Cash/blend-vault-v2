#![cfg(test)]

use crate::storage::ONE_DAY_LEDGERS;
use crate::testutils::{create_funded_blend_vault, EnvTestUtils};
use crate::BlendVaultClient;
use blend_contract_sdk::pool::Client as PoolClient;
use sep_41_token::testutils::MockTokenClient;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, Error};

const DEPOSIT: i128 = 100_0000000;

/// Builds a vault with two funded depositors, then lets a week of interest
/// accrue so the bRate is an awkward number and the share/bToken/underlying
/// conversions actually round.
///
/// Returns (vault address, usdc address, samwise, frodo).
fn setup(e: &Env) -> (Address, Address, Address, Address) {
    let (vault, usdc) = create_funded_blend_vault(e);
    let usdc_client = MockTokenClient::new(e, &usdc);
    let vault_client = BlendVaultClient::new(e, &vault);

    let samwise = Address::generate(e);
    let frodo = Address::generate(e);
    usdc_client.mint(&samwise, &DEPOSIT);
    usdc_client.mint(&frodo, &(DEPOSIT * 3));
    vault_client.deposit(&DEPOSIT, &samwise, &samwise, &samwise);
    vault_client.deposit(&(DEPOSIT * 3), &frodo, &frodo, &frodo);

    e.jump(ONE_DAY_LEDGERS * 7);

    (vault, usdc, samwise, frodo)
}

/// The property YBC asserts on every zap: redeeming an exact share count leaves
/// the owner's share balance exactly `balance - shares`, with no dust either
/// way. A full redeem lands on precisely zero.
#[test]
fn test_redeem_burns_exact_shares() {
    let e = Env::default();
    let (vault, _usdc, samwise, _frodo) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);

    let start = vault_client.get_shares(&samwise);
    assert!(start > 0);

    // an odd, non-round share count so nothing divides cleanly
    let partial = start / 3 + 7;
    vault_client.redeem(&partial, &samwise, &samwise, &samwise);
    assert_eq!(vault_client.get_shares(&samwise), start - partial);

    let remaining = vault_client.get_shares(&samwise);
    vault_client.redeem(&remaining, &samwise, &samwise, &samwise);
    assert_eq!(vault_client.get_shares(&samwise), 0);
}

/// Verifies the `underlying_to_b_tokens_up` recompute: after a redeem, the
/// vault's own `total_b_tokens` must still equal the position the pool actually
/// holds for it. Rounding down here would leave the vault claiming more bTokens
/// than it owns.
#[test]
fn test_redeem_accounting_matches_pool_position() {
    let e = Env::default();
    let (vault, _usdc, samwise, _frodo) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);
    let (pool, asset) = vault_client.get_config();
    let pool_client = PoolClient::new(&e, &pool);
    let reserve_index = pool_client.get_reserve(&asset).config.index;

    let pool_position = || {
        pool_client
            .get_positions(&vault)
            .supply
            .get(reserve_index)
            .unwrap_or(0)
    };

    assert_eq!(vault_client.get_vault().total_b_tokens, pool_position());

    let shares = vault_client.get_shares(&samwise);
    vault_client.redeem(&(shares / 3 + 7), &samwise, &samwise, &samwise);
    assert_eq!(vault_client.get_vault().total_b_tokens, pool_position());

    vault_client.redeem(
        &vault_client.get_shares(&samwise),
        &samwise,
        &samwise,
        &samwise,
    );
    assert_eq!(vault_client.get_vault().total_b_tokens, pool_position());
}

/// Redeem pays out the down-rounded value of the shares and never more.
#[test]
fn test_redeem_never_over_delivers() {
    let e = Env::default();
    let (vault, usdc, samwise, _frodo) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);
    let usdc_client = MockTokenClient::new(&e, &usdc);

    let shares = vault_client.get_shares(&samwise);
    let quoted = vault_client.convert_to_assets(&shares);

    let assets = vault_client.redeem(&shares, &samwise, &samwise, &samwise);

    // `convert_to_assets` is the quote YBC's yield manager tracks, so redeem
    // has to land on it exactly rather than merely near it
    assert_eq!(assets, quoted);
    assert_eq!(usdc_client.balance(&samwise), assets);
}

/// Unlike `withdraw`, an over-large request is an error rather than a hint to
/// burn everything — the caller named an exact share count.
#[test]
fn test_redeem_over_balance_fails() {
    let e = Env::default();
    let (vault, _usdc, samwise, _frodo) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);

    let shares = vault_client.get_shares(&samwise);

    let result = vault_client.try_redeem(&(shares + 1), &samwise, &samwise, &samwise);
    assert_eq!(result.err(), Some(Ok(Error::from_contract_error(10))));
    // the failed attempt left the position untouched
    assert_eq!(vault_client.get_shares(&samwise), shares);

    // ...whereas withdraw still clamps down, which YBC relies on staying put
    vault_client.withdraw(&(i64::MAX as i128), &samwise, &samwise, &samwise);
    assert_eq!(vault_client.get_shares(&samwise), 0);
}

/// Non-positive share counts are rejected before anything moves.
#[test]
fn test_redeem_non_positive_shares_fails() {
    let e = Env::default();
    let (vault, _usdc, samwise, _frodo) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);

    for shares in [0i128, -1i128] {
        let result = vault_client.try_redeem(&shares, &samwise, &samwise, &samwise);
        assert_eq!(result.err(), Some(Ok(Error::from_contract_error(112))));
    }
}

/// Redeeming does not disturb other holders' positions, and any rounding it
/// leaves behind may only ever favor them.
#[test]
fn test_redeem_leaves_other_holders_whole() {
    let e = Env::default();
    let (vault, _usdc, samwise, frodo) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);

    let frodo_shares = vault_client.get_shares(&frodo);
    let frodo_value_before = vault_client.get_underlying_tokens(&frodo);

    vault_client.redeem(
        &vault_client.get_shares(&samwise),
        &samwise,
        &samwise,
        &samwise,
    );

    assert_eq!(vault_client.get_shares(&frodo), frodo_shares);
    assert!(vault_client.get_underlying_tokens(&frodo) >= frodo_value_before);
}

/// A contract, not just a classic account, can receive the proceeds — nothing
/// in the payout path assumes an account-backed address.
#[test]
fn test_redeem_to_contract_receiver() {
    let e = Env::default();
    let (vault, usdc, samwise, _frodo) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);
    let usdc_client = MockTokenClient::new(&e, &usdc);

    let receiver = vault.clone();
    let receiver_before = usdc_client.balance(&receiver);

    let shares = vault_client.get_shares(&samwise);
    let assets = vault_client.redeem(&shares, &receiver, &samwise, &samwise);

    assert!(assets > 0);
    assert_eq!(usdc_client.balance(&receiver), receiver_before + assets);
    assert_eq!(vault_client.get_shares(&samwise), 0);
}