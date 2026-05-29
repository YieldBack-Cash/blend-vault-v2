#![cfg(test)]

use crate::{
    storage,
    testutils::{
        assert_approx_eq_rel, create_blend_pool, mockpool, register_fee_vault, EnvTestUtils,
    },
    vault::VaultData,
    FeeVaultClient,
};
use blend_contract_sdk::testutils::BlendFixture;
use sep_41_token::testutils::MockTokenClient;
use soroban_sdk::{contractclient, testutils::Address as _, Address, Env};

/// OZ-style ERC-4626 interface used to generate VaultContractClient.
///
/// The client dispatches calls by function name on whatever contract address is
/// passed. Functions whose name AND argument types match FeeVault's on-chain ABI
/// will succeed; mismatched signatures will return an error at the host level.
#[contractclient(name = "VaultContractClient")]
pub trait VaultTrait {
    fn __constructor(e: Env, asset: Address, decimals_offset: u32, strategy: Address);
    fn convert_to_assets(e: &Env, shares: i128) -> i128;
    fn deposit(
        e: &Env,
        assets: i128,
        receiver: Address,
        from: Address,
        operator: Address,
    ) -> i128;
    fn withdraw(
        e: &Env,
        assets: i128,
        receiver: Address,
        owner: Address,
        operator: Address,
    ) -> i128;
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Sets up a full Blend pool + fee vault with usdc as the asset.
/// Returns (vault_address, usdc_client, frodo, samwise).
fn setup_blend(e: &Env) -> (Address, MockTokenClient, Address, Address) {
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let bombadil = Address::generate(e);
    let frodo = Address::generate(e);
    let samwise = Address::generate(e);

    let blnd = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let usdc = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let xlm = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let usdc_client = MockTokenClient::new(e, &usdc);
    let xlm_client = MockTokenClient::new(e, &xlm);

    let blend_fixture = BlendFixture::deploy(e, &bombadil, &blnd, &usdc);
    let pool = create_blend_pool(e, &blend_fixture, &bombadil, &usdc_client, &xlm_client);

    let vault = register_fee_vault(e, &bombadil, &pool, &usdc, None);

    usdc_client.mint(&frodo, &10_000_0000000);
    usdc_client.mint(&samwise, &10_000_0000000);

    (vault, usdc_client, frodo, samwise)
}

/// Sets up a vault with directly-written storage state (no real pool).
/// Returns (vault_address, pool_address, samwise, frodo).
fn setup_mock(e: &Env) -> (Address, Address, Address, Address) {
    let admin = Address::generate(e);
    let samwise = Address::generate(e);
    let frodo = Address::generate(e);

    let b_rate = 1_000_000_000_000_i128;
    let pool = mockpool::register_mock_pool_with_b_rate(e, b_rate).address;
    let reserve = Address::generate(e);
    let vault = register_fee_vault(e, &admin, &pool, &reserve, None);

    e.as_contract(&vault, || {
        storage::set_vault_data(
            e,
            &VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate,
                last_update_timestamp: e.ledger().timestamp(),
            },
        );
        // samwise: 10 %, frodo: 90 %
        storage::set_vault_shares(e, &samwise, 120_0000000);
        storage::set_vault_shares(e, &frodo, 1080_0000000);
    });

    (vault, pool, samwise, frodo)
}

// ── convert_to_assets ─────────────────────────────────────────────────────────

/// VaultContractClient::convert_to_assets produces the same result as
/// FeeVaultClient::get_underlying_tokens for each user's share balance.
#[test]
fn test_convert_to_assets_matches_underlying_tokens() {
    let e = Env::default();
    e.mock_all_auths();
    e.set_default_info();

    let (vault, _pool, samwise, frodo) = setup_mock(&e);
    let fee_client = FeeVaultClient::new(&e, &vault);
    let oz_client = VaultContractClient::new(&e, &vault);

    let samwise_shares = fee_client.get_shares(&samwise);
    let frodo_shares = fee_client.get_shares(&frodo);

    assert_eq!(
        oz_client.convert_to_assets(&samwise_shares),
        fee_client.get_underlying_tokens(&samwise)
    );
    assert_eq!(
        oz_client.convert_to_assets(&frodo_shares),
        fee_client.get_underlying_tokens(&frodo)
    );
}

/// Both clients call the same on-chain function, so they must always agree for
/// any arbitrary share amount.
#[test]
fn test_convert_to_assets_agrees_with_fee_vault_client() {
    let e = Env::default();
    e.mock_all_auths();
    e.set_default_info();

    let (vault, _pool, _samwise, _frodo) = setup_mock(&e);
    let fee_client = FeeVaultClient::new(&e, &vault);
    let oz_client = VaultContractClient::new(&e, &vault);

    for shares in [0_i128, 1, 120_0000000, 600_0000000, 1080_0000000, 1200_0000000] {
        assert_eq!(
            oz_client.convert_to_assets(&shares),
            fee_client.convert_to_assets(&shares),
            "mismatch at shares = {}",
            shares
        );
    }
}

/// convert_to_assets must reflect the current b_rate, not a stale cached value.
#[test]
fn test_convert_to_assets_reflects_rate_change() {
    let e = Env::default();
    e.mock_all_auths();
    e.set_default_info();

    let (vault, pool, samwise, _frodo) = setup_mock(&e);
    let fee_client = FeeVaultClient::new(&e, &vault);
    let oz_client = VaultContractClient::new(&e, &vault);

    let shares = fee_client.get_shares(&samwise);
    let before = oz_client.convert_to_assets(&shares);

    let mock_client = mockpool::MockPoolClient::new(&e, &pool);
    mock_client.set_b_rate(&2_000_000_000_000_i128);
    e.jump(5);

    let after = oz_client.convert_to_assets(&shares);

    // depositors keep 100 % of the 100 % gain → 200 % of original
    assert_approx_eq_rel(after, before * 2, 0_0000001);
}

/// 0 shares must always return 0.
#[test]
fn test_convert_to_assets_zero_shares() {
    let e = Env::default();
    e.mock_all_auths();
    e.set_default_info();

    let (vault, _pool, _samwise, _frodo) = setup_mock(&e);
    assert_eq!(VaultContractClient::new(&e, &vault).convert_to_assets(&0), 0);
}

// ── deposit ───────────────────────────────────────────────────────────────────

/// VaultContractClient::deposit now matches FeeVault::deposit's signature
/// and can execute a real deposit end-to-end.
#[test]
fn test_deposit_via_contract_client() {
    let e = Env::default();
    let (vault, _usdc, frodo, _samwise) = setup_blend(&e);

    let fee_client = FeeVaultClient::new(&e, &vault);
    let oz_client = VaultContractClient::new(&e, &vault);

    let deposit_amount = 1_000_0000000_i128;

    // frodo is both the asset provider and the share receiver
    let shares_minted = oz_client.deposit(&deposit_amount, &frodo, &frodo, &frodo);

    assert!(shares_minted > 0);
    assert_eq!(fee_client.get_shares(&frodo), shares_minted);
    assert_eq!(
        oz_client.convert_to_assets(&shares_minted),
        fee_client.get_underlying_tokens(&frodo)
    );
}

/// VaultContractClient::deposit supports a split receiver/from: frodo provides
/// the assets but samwise receives the shares.
#[test]
fn test_deposit_split_receiver_and_from() {
    let e = Env::default();
    let (vault, _usdc, frodo, samwise) = setup_blend(&e);

    let fee_client = FeeVaultClient::new(&e, &vault);
    let oz_client = VaultContractClient::new(&e, &vault);

    let deposit_amount = 1_000_0000000_i128;

    // frodo pays, samwise receives shares, frodo is the operator
    let shares_minted = oz_client.deposit(&deposit_amount, &samwise, &frodo, &frodo);

    assert!(shares_minted > 0);
    assert_eq!(fee_client.get_shares(&samwise), shares_minted);
    assert_eq!(fee_client.get_shares(&frodo), 0);
}

// ── withdraw ──────────────────────────────────────────────────────────────────

/// VaultContractClient::withdraw matches FeeVault::withdraw's signature
/// and can execute a real withdrawal end-to-end.
#[test]
fn test_withdraw_via_contract_client() {
    let e = Env::default();
    let (vault, usdc, frodo, _samwise) = setup_blend(&e);

    let fee_client = FeeVaultClient::new(&e, &vault);
    let oz_client = VaultContractClient::new(&e, &vault);

    let deposit_amount = 1_000_0000000_i128;
    fee_client.deposit(&deposit_amount, &frodo, &frodo, &frodo);

    let balance_before = usdc.balance(&frodo);
    let withdraw_amount = 500_0000000_i128;

    let shares_burned = oz_client.withdraw(&withdraw_amount, &frodo, &frodo, &frodo);

    assert!(shares_burned > 0);
    assert!(usdc.balance(&frodo) > balance_before);
}

/// VaultContractClient::withdraw supports a split receiver/owner: samwise owns
/// the shares but frodo receives the underlying tokens.
#[test]
fn test_withdraw_split_receiver_and_owner() {
    let e = Env::default();
    let (vault, usdc, frodo, samwise) = setup_blend(&e);

    let fee_client = FeeVaultClient::new(&e, &vault);
    let oz_client = VaultContractClient::new(&e, &vault);

    let deposit_amount = 1_000_0000000_i128;
    fee_client.deposit(&deposit_amount, &samwise, &samwise, &samwise);

    let frodo_balance_before = usdc.balance(&frodo);
    let samwise_balance_before = usdc.balance(&samwise);

    // samwise burns shares, frodo receives the underlying
    oz_client.withdraw(&500_0000000_i128, &frodo, &samwise, &samwise);

    assert!(usdc.balance(&frodo) > frodo_balance_before);
    assert_eq!(usdc.balance(&samwise), samwise_balance_before);
}