#![cfg(test)]

use crate::testutils::{
    assert_approx_eq_abs, create_blend_pool, register_blend_vault, setup_pool_util_rate,
    EnvTestUtils,
};
use crate::BlendVaultClient;
use blend_contract_sdk::testutils::BlendFixture;
use sep_41_token::testutils::MockTokenClient;
use soroban_sdk::{testutils::Address as _, Address, Env};

// ── fixture ───────────────────────────────────────────────────────────────────

struct Fixture<'a> {
    blend_vault_client: BlendVaultClient<'a>,
    usdc: Address,
    frodo: Address,
    samwise: Address,
}

fn setup(e: &Env, frodo_deposit: i128) -> Fixture<'_> {
    let bombadil = Address::generate(e);
    let gandalf = Address::generate(e);
    let frodo = Address::generate(e);
    let samwise = Address::generate(e);

    let blnd = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let usdc = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let xlm = e.register_stellar_asset_contract_v2(bombadil.clone()).address();

    let usdc_client = MockTokenClient::new(e, &usdc);

    let blend_fixture = BlendFixture::deploy(e, &bombadil, &blnd, &usdc);
    let pool = create_blend_pool(
        e,
        &blend_fixture,
        &bombadil,
        &usdc_client,
        &MockTokenClient::new(e, &xlm),
    );

    // bombadil supplies + borrows to hold 50% util rate → ~5% supply APR
    setup_pool_util_rate(e, &pool, &bombadil, &usdc, &xlm, 100_000_0000000);

    let blend_vault = register_blend_vault(e, &gandalf, &pool, &usdc, &blnd);
    let blend_vault_client = BlendVaultClient::new(e, &blend_vault);

    usdc_client.mint(&frodo, &frodo_deposit);
    blend_vault_client.deposit(&frodo_deposit, &frodo, &frodo, &frodo);

    Fixture { blend_vault_client, usdc, frodo, samwise }
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Transferring shares moves the correct underlying token value to the receiver.
/// Both parties can withdraw their position after the transfer.
#[test]
fn test_transfer_splits_underlying_value() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let deposit: i128 = 100_0000000;
    let f = setup(&e, deposit);
    let usdc_client = MockTokenClient::new(&e, &f.usdc);

    // let yield accrue for a week
    e.jump(crate::storage::ONE_DAY_LEDGERS * 7);

    let shares_total = f.blend_vault_client.get_shares(&f.frodo);
    let underlying_before = f.blend_vault_client.get_underlying_tokens(&f.frodo);
    assert!(underlying_before > deposit, "b_rate yield must have accrued");

    // transfer half the shares to samwise
    let transfer_amount = shares_total / 2;
    f.blend_vault_client.transfer(&f.frodo, &f.samwise, &transfer_amount);

    // share counts are correct
    let frodo_shares = f.blend_vault_client.get_shares(&f.frodo);
    let samwise_shares = f.blend_vault_client.get_shares(&f.samwise);
    assert_eq!(frodo_shares + samwise_shares, shares_total);

    // underlying value splits proportionally (within 1 stroop rounding)
    let frodo_underlying = f.blend_vault_client.get_underlying_tokens(&f.frodo);
    let samwise_underlying = f.blend_vault_client.get_underlying_tokens(&f.samwise);
    assert_approx_eq_abs(frodo_underlying, underlying_before / 2, 2);
    assert_approx_eq_abs(samwise_underlying, underlying_before / 2, 2);

    // both parties can withdraw their full position
    f.blend_vault_client.withdraw(&(frodo_underlying * 2), &f.frodo, &f.frodo, &f.frodo);
    f.blend_vault_client.withdraw(&(samwise_underlying * 2), &f.samwise, &f.samwise, &f.samwise);

    assert_approx_eq_abs(usdc_client.balance(&f.frodo), frodo_underlying, 2);
    assert_approx_eq_abs(usdc_client.balance(&f.samwise), samwise_underlying, 2);
    assert_eq!(f.blend_vault_client.get_shares(&f.frodo), 0);
    assert_eq!(f.blend_vault_client.get_shares(&f.samwise), 0);
}

/// After a transfer, yield from b_rate appreciation continues to accrue
/// proportionally to whoever holds the shares.
#[test]
fn test_yield_accrues_to_share_holder_after_transfer() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let f = setup(&e, 100_0000000);

    // transfer ALL shares to samwise before any yield accrues
    let shares = f.blend_vault_client.get_shares(&f.frodo);
    f.blend_vault_client.transfer(&f.frodo, &f.samwise, &shares);

    assert_eq!(f.blend_vault_client.get_shares(&f.frodo), 0);
    assert_eq!(f.blend_vault_client.get_underlying_tokens(&f.frodo), 0);

    // let yield accrue for a week
    e.jump(crate::storage::ONE_DAY_LEDGERS * 7);

    // samwise holds all shares so all yield goes to samwise
    let samwise_underlying = f.blend_vault_client.get_underlying_tokens(&f.samwise);
    assert!(samwise_underlying > 100_0000000, "samwise must earn yield");
    assert_eq!(f.blend_vault_client.get_underlying_tokens(&f.frodo), 0);
}

/// Transferring shares then receiving more deposits doesn't corrupt the
/// share accounting: total_shares always equals the sum of individual balances.
#[test]
fn test_total_shares_consistent_after_transfer_and_deposit() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let deposit: i128 = 100_0000000;
    let f = setup(&e, deposit);
    let usdc_client = MockTokenClient::new(&e, &f.usdc);

    let merry = Address::generate(&e);
    usdc_client.mint(&merry, &deposit);

    // frodo transfers half to samwise
    let frodo_shares = f.blend_vault_client.get_shares(&f.frodo);
    f.blend_vault_client.transfer(&f.frodo, &f.samwise, &(frodo_shares / 2));

    // merry deposits
    f.blend_vault_client.deposit(&deposit, &merry, &merry, &merry);

    let total = f.blend_vault_client.get_vault().total_shares;
    let sum = f.blend_vault_client.get_shares(&f.frodo)
        + f.blend_vault_client.get_shares(&f.samwise)
        + f.blend_vault_client.get_shares(&merry);

    assert_eq!(total, sum);
}

/// Shares can be chained through multiple transfers and the final holder
/// can withdraw the full underlying position.
#[test]
fn test_chain_of_transfers_final_holder_can_withdraw() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let f = setup(&e, 100_0000000);
    let usdc_client = MockTokenClient::new(&e, &f.usdc);
    let merry = Address::generate(&e);

    e.jump(crate::storage::ONE_DAY_LEDGERS * 3);

    // frodo → samwise
    let frodo_shares = f.blend_vault_client.get_shares(&f.frodo);
    f.blend_vault_client.transfer(&f.frodo, &f.samwise, &frodo_shares);

    e.jump(crate::storage::ONE_DAY_LEDGERS * 3);

    // samwise → merry
    let samwise_shares = f.blend_vault_client.get_shares(&f.samwise);
    f.blend_vault_client.transfer(&f.samwise, &merry, &samwise_shares);

    e.jump(crate::storage::ONE_DAY_LEDGERS * 3);

    assert_eq!(f.blend_vault_client.get_shares(&f.frodo), 0);
    assert_eq!(f.blend_vault_client.get_shares(&f.samwise), 0);

    // merry holds everything and can withdraw
    let merry_underlying = f.blend_vault_client.get_underlying_tokens(&merry);
    assert!(merry_underlying > 100_0000000);
    f.blend_vault_client.withdraw(&(merry_underlying * 2), &merry, &merry, &merry);
    assert_eq!(f.blend_vault_client.get_shares(&merry), 0);
    assert!(usdc_client.balance(&merry) > 100_0000000);
}