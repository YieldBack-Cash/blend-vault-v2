#![cfg(test)]

use crate::storage::ONE_DAY_LEDGERS;
use crate::testutils::{create_blend_pool, mocksoroswap, register_fee_vault, EnvTestUtils};
use crate::{storage, FeeVaultClient};
use blend_contract_sdk::pool::{Client as PoolClient, Request};
use blend_contract_sdk::testutils::BlendFixture;
use sep_41_token::testutils::MockTokenClient;
use soroban_sdk::{testutils::Address as _, vec, Address, Env};

/// Full claim_emissions flow using a mock Soroswap router.
///
/// Emission cycle:
///   1. create_blend_pool sets baseline via emitter.distribute + backstop.distribute (returns 0)
///   2. Vault accrues a supply position
///   3. Jump 7 days → second distribute cycle → backstop.distribute writes rz_emis.accrued > 0
///   4. gulp_emissions → backstop sets eps for next 7 days (rate, not lump sum)
///   5. Jump 3 more days → eps × 3 days accumulates in the emission index
///   6. claim_emissions: pool.claim (BLND) → soroswap swap (BLND→USDC) → pool.supply → more b_tokens
#[test]
fn test_claim_emissions_swaps_blnd_for_underlying() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let bombadil = Address::generate(&e);
    let frodo = Address::generate(&e);

    let blnd = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let usdc = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let xlm = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let usdc_client = MockTokenClient::new(&e, &usdc);
    let xlm_client = MockTokenClient::new(&e, &xlm);

    let blend_fixture = BlendFixture::deploy(&e, &bombadil, &blnd, &usdc);
    let pool = create_blend_pool(&e, &blend_fixture, &bombadil, &usdc_client, &xlm_client);
    let pool_client = PoolClient::new(&e, &pool);

    // Register mock router and pre-fund it with USDC so it can pay out swaps 1:1
    let router_client = mocksoroswap::register_mock_soroswap_router(&e);
    let router = router_client.address.clone();
    usdc_client.mint(&router, &10_000_000_0000000);

    let vault = register_fee_vault(&e, &bombadil, &pool, &usdc, None, Some(router));
    let fee_vault_client = FeeVaultClient::new(&e, &vault);

    // Override BLND token so the vault uses our test BLND instead of the hardcoded mainnet address
    e.as_contract(&vault, || {
        storage::set_blnd_token_for_test(&e, &blnd);
    });

    // Establish pool liquidity so the vault's supply position accrues against real utilisation
    pool_client.submit(
        &bombadil,
        &bombadil,
        &bombadil,
        &vec![
            &e,
            Request { address: usdc.clone(), amount: 200_000_0000000, request_type: 2 },
            Request { address: usdc.clone(), amount: 100_000_0000000, request_type: 4 },
            Request { address: xlm.clone(),  amount: 200_000_0000000, request_type: 2 },
            Request { address: xlm.clone(),  amount: 100_000_0000000, request_type: 4 },
        ],
    );

    // Frodo deposits into vault — vault now has a supply position in the pool
    let deposit = 10_000_0000000_i128;
    usdc_client.mint(&frodo, &deposit);
    fee_vault_client.deposit(&deposit, &frodo, &frodo, &frodo);

    let pool_position_before = pool_client
        .get_positions(&vault)
        .supply
        .get(0)
        .unwrap_or(0);
    assert!(pool_position_before > 0, "vault must have a pool supply position before claiming");

    let b_tokens_before = fee_vault_client.get_vault().total_b_tokens;

    // -- Second emission cycle (create_blend_pool already ran the first/baseline cycle) --
    // emitter.distribute mints BLND to the backstop for the 7-day elapsed period.
    // backstop.distribute allocates that to each reward-zone pool's rz_emis.accrued.
    // pool.gulp_emissions consumes rz_emis.accrued and sets the emission rate (eps) for
    // the NEXT 7 days — it does NOT give a lump sum; the eps accumulates over time.
    e.jump(ONE_DAY_LEDGERS * 7);
    blend_fixture.emitter.distribute();
    blend_fixture.backstop.distribute();
    pool_client.gulp_emissions();

    // Let 3 days of eps accumulate in the pool's emission index before claiming
    e.jump(ONE_DAY_LEDGERS * 3);

    // claim_emissions: pool.claim(BLND) → soroswap swap(BLND→USDC) → pool.supply → b_tokens
    let underlying_received = fee_vault_client.claim_emissions(&0);
    assert!(
        underlying_received > 0,
        "claim_emissions must return > 0 underlying after emissions accumulated: got {}",
        underlying_received
    );

    let b_tokens_after = fee_vault_client.get_vault().total_b_tokens;
    assert!(
        b_tokens_after > b_tokens_before,
        "vault bToken balance must grow after harvest: before={}, after={}",
        b_tokens_before,
        b_tokens_after,
    );

    let pool_position_after = pool_client
        .get_positions(&vault)
        .supply
        .get(0)
        .unwrap_or(0);
    assert!(
        pool_position_after > pool_position_before,
        "pool bToken position must grow: before={}, after={}",
        pool_position_before,
        pool_position_after,
    );
}

/// If no BLND has accrued (vault has never had a supply position), claim_emissions
/// should return 0 without panicking.
#[test]
fn test_claim_emissions_zero_blnd_returns_zero() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let bombadil = Address::generate(&e);

    let blnd = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let usdc = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let xlm = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let usdc_client = MockTokenClient::new(&e, &usdc);
    let xlm_client = MockTokenClient::new(&e, &xlm);

    let blend_fixture = BlendFixture::deploy(&e, &bombadil, &blnd, &usdc);
    let pool = create_blend_pool(&e, &blend_fixture, &bombadil, &usdc_client, &xlm_client);

    let router_client = mocksoroswap::register_mock_soroswap_router(&e);
    let router = router_client.address.clone();

    let vault = register_fee_vault(&e, &bombadil, &pool, &usdc, None, Some(router));
    let fee_vault_client = FeeVaultClient::new(&e, &vault);

    e.as_contract(&vault, || {
        storage::set_blnd_token_for_test(&e, &blnd);
    });

    // no deposit → no supply position → no BLND accrued
    let result = fee_vault_client.claim_emissions(&0);
    assert_eq!(result, 0);
}