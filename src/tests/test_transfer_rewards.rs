#![cfg(test)]

use crate::testutils::{assert_approx_eq_abs, create_blend_pool, register_fee_vault, EnvTestUtils};
use crate::FeeVaultClient;
use blend_contract_sdk::pool::{Client as PoolClient, Request};
use blend_contract_sdk::testutils::BlendFixture;
use sep_41_token::testutils::MockTokenClient;
use soroban_sdk::{testutils::Address as _, vec, Address, Env};

// ── fixture ───────────────────────────────────────────────────────────────────

struct Fixture<'a> {
    fee_vault_client: FeeVaultClient<'a>,
    usdc: Address,
    xlm: Address,
    frodo: Address,
    samwise: Address,
}

/// Full Blend pool + fee vault.
///
/// - 50% utilisation → ~5% supply APR (10% borrow rate, 0% backstop take)
/// - XLM reward period of `reward_period` seconds starts immediately
/// - Frodo has deposited `frodo_deposit` USDC and owns 100% of vault shares
/// - No time has elapsed yet
fn setup<'a>(
    e: &'a Env,
    frodo_deposit: i128,
    xlm_rewards: i128,
    reward_period: u64,
) -> Fixture<'a> {
    let bombadil = Address::generate(e);
    let gandalf = Address::generate(e);
    let frodo = Address::generate(e);
    let samwise = Address::generate(e);

    let blnd = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let usdc = e.register_stellar_asset_contract_v2(bombadil.clone()).address();
    let xlm = e.register_stellar_asset_contract_v2(bombadil.clone()).address();

    let usdc_client = MockTokenClient::new(e, &usdc);
    let xlm_client = MockTokenClient::new(e, &xlm);

    let blend_fixture = BlendFixture::deploy(e, &bombadil, &blnd, &usdc);
    let pool = create_blend_pool(
        e,
        &blend_fixture,
        &bombadil,
        &usdc_client,
        &MockTokenClient::new(e, &xlm),
    );
    let pool_client = PoolClient::new(e, &pool);

    // bombadil supplies + borrows to hold 50% util rate
    pool_client.mock_all_auths().submit(
        &bombadil,
        &bombadil,
        &bombadil,
        &vec![
            e,
            Request { address: usdc.clone(), amount: 200_000_0000000, request_type: 2 },
            Request { address: usdc.clone(), amount: 100_000_0000000, request_type: 4 },
            Request { address: xlm.clone(),  amount: 200_000_0000000, request_type: 2 },
            Request { address: xlm.clone(),  amount: 100_000_0000000, request_type: 4 },
        ],
    );

    // 10% take-rate fee vault (admin earns 10% of yield)
    let fee_vault = register_fee_vault(e, &gandalf, &pool, &usdc, 0, 100_0000, None);
    let fee_vault_client = FeeVaultClient::new(e, &fee_vault);

    usdc_client.mint(&frodo, &frodo_deposit);
    fee_vault_client.deposit(&frodo_deposit, &frodo, &frodo, &frodo);

    xlm_client.mint(&gandalf, &xlm_rewards);
    fee_vault_client.set_rewards(
        &xlm,
        &xlm_rewards,
        &(e.ledger().timestamp() + reward_period),
    );

    Fixture { fee_vault_client, usdc, xlm, frodo, samwise }
}

// ── integration test ──────────────────────────────────────────────────────────

/// Complete fungibility integration test.
///
/// Verifies that a mid-period share transfer leaves:
///   - share balances correct (sum unchanged, individual balances split)
///   - underlying token value transferred proportionally (receiver can withdraw it)
///   - yield from b_rate appreciation allocated to whoever holds shares
///   - vault rewards (XLM) split at the transfer boundary (sender 75%, receiver 25%)
///   - both parties can withdraw the correct USDC amount
#[test]
fn test_transfer_fungibility_integration() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let reward_period: u64 = 100_000;
    let xlm_rewards: i128 = 10_000_0000000;
    let deposit: i128 = 100_0000000;
    let f = setup(&e, deposit, xlm_rewards, reward_period);

    let usdc_client = MockTokenClient::new(&e, &f.usdc);
    let xlm_client = MockTokenClient::new(&e, &f.xlm);

    // ── before transfer ───────────────────────────────────────────────────────
    // Advance half the reward period; pool b_rate yield also accrues.
    e.jump_time(reward_period / 2);

    let shares_before_transfer = f.fee_vault_client.get_shares(&f.frodo);
    let underlying_at_transfer = f.fee_vault_client.get_underlying_tokens(&f.frodo);
    // b_rate must have grown, making the position worth more than the deposit
    assert!(underlying_at_transfer > deposit, "b_rate yield must have accrued");

    // ── transfer half the shares ──────────────────────────────────────────────
    let transfer_amount = shares_before_transfer / 2;
    f.fee_vault_client.transfer(&f.frodo, &f.samwise, &transfer_amount);

    // share balances update immediately and correctly
    let frodo_shares_after = f.fee_vault_client.get_shares(&f.frodo);
    let samwise_shares_after = f.fee_vault_client.get_shares(&f.samwise);
    assert_eq!(frodo_shares_after + samwise_shares_after, shares_before_transfer);
    assert_eq!(frodo_shares_after, transfer_amount); // exactly half

    // underlying token value splits proportionally with shares
    let frodo_underlying_after = f.fee_vault_client.get_underlying_tokens(&f.frodo);
    let samwise_underlying_after = f.fee_vault_client.get_underlying_tokens(&f.samwise);
    assert_approx_eq_abs(frodo_underlying_after, underlying_at_transfer / 2, 2);
    assert_approx_eq_abs(samwise_underlying_after, underlying_at_transfer / 2, 2);

    // ── second half of reward period ──────────────────────────────────────────
    // Both hold equal shares so each earns 50% of the remaining yield and rewards.
    e.jump_time(reward_period / 2);

    let frodo_underlying_final = f.fee_vault_client.get_underlying_tokens(&f.frodo);
    let samwise_underlying_final = f.fee_vault_client.get_underlying_tokens(&f.samwise);
    assert!(frodo_underlying_final > frodo_underlying_after, "yield must keep accruing for frodo");
    assert!(samwise_underlying_final > samwise_underlying_after, "yield must keep accruing for samwise");
    // equal shares → equal underlying at end
    assert_approx_eq_abs(frodo_underlying_final, samwise_underlying_final, 2);

    // ── XLM vault rewards ─────────────────────────────────────────────────────
    // frodo held 100% for first half → 5 000 XLM
    // frodo held  50% for second half → 2 500 XLM  total: 7 500 (75%)
    // samwise held 0% for first half, 50% for second half → 2 500 XLM (25%)
    let frodo_xlm = f.fee_vault_client.claim_rewards(&f.frodo, &f.xlm, &f.frodo);
    let samwise_xlm = f.fee_vault_client.claim_rewards(&f.samwise, &f.xlm, &f.samwise);

    assert_approx_eq_abs(frodo_xlm,  7500_0000000, 1_0000000);
    assert_approx_eq_abs(samwise_xlm, 2500_0000000, 1_0000000);
    assert_approx_eq_abs(frodo_xlm + samwise_xlm, xlm_rewards, 1_0000000);
    assert_eq!(xlm_client.balance(&f.frodo), frodo_xlm);
    assert_eq!(xlm_client.balance(&f.samwise), samwise_xlm);

    // ── withdrawal ────────────────────────────────────────────────────────────
    // Over-request withdrawal; vault caps it at the actual balance (same pattern
    // as the happy path test), which avoids a 1-share dust residual from rounding.
    f.fee_vault_client.withdraw(&(frodo_underlying_final * 2), &f.frodo, &f.frodo, &f.frodo);
    f.fee_vault_client.withdraw(&(samwise_underlying_final * 2), &f.samwise, &f.samwise, &f.samwise);

    assert_approx_eq_abs(usdc_client.balance(&f.frodo), frodo_underlying_final, 2);
    assert_approx_eq_abs(usdc_client.balance(&f.samwise), samwise_underlying_final, 2);

    // combined withdrawal must exceed the original deposit (net of 10% admin fee)
    assert!(usdc_client.balance(&f.frodo) + usdc_client.balance(&f.samwise) > deposit);

    assert_eq!(f.fee_vault_client.get_shares(&f.frodo), 0);
    assert_eq!(f.fee_vault_client.get_shares(&f.samwise), 0);
}

// ── focused edge cases ────────────────────────────────────────────────────────

/// Samwise receives all shares after 90% of the reward period has elapsed.
/// He must not be able to claim rewards from before the transfer.
/// He does receive the full underlying token value and can withdraw it.
#[test]
fn test_receiver_earns_no_retroactive_rewards() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let reward_period: u64 = 100_000;
    let f = setup(&e, 100_0000000, 10_000_0000000, reward_period);
    let xlm_client = MockTokenClient::new(&e, &f.xlm);

    e.jump_time(reward_period * 9 / 10);

    let frodo_shares = f.fee_vault_client.get_shares(&f.frodo);
    f.fee_vault_client.transfer(&f.frodo, &f.samwise, &frodo_shares);

    e.jump_time(reward_period / 10);

    let frodo_xlm = f.fee_vault_client.claim_rewards(&f.frodo, &f.xlm, &f.frodo);
    let samwise_xlm = f.fee_vault_client.claim_rewards(&f.samwise, &f.xlm, &f.samwise);

    assert_approx_eq_abs(frodo_xlm,   9000_0000000, 1_0000000);
    assert_approx_eq_abs(samwise_xlm, 1000_0000000, 1_0000000);
    assert!(samwise_xlm < frodo_xlm);
    assert_eq!(xlm_client.balance(&f.frodo), frodo_xlm);
    assert_eq!(xlm_client.balance(&f.samwise), samwise_xlm);

    // samwise holds all shares; he can withdraw the full underlying position
    let samwise_underlying = f.fee_vault_client.get_underlying_tokens(&f.samwise);
    assert!(samwise_underlying > 0);
    f.fee_vault_client.withdraw(&samwise_underlying, &f.samwise, &f.samwise, &f.samwise);
    assert_eq!(f.fee_vault_client.get_shares(&f.samwise), 0);
    assert_eq!(f.fee_vault_client.get_underlying_tokens(&f.frodo), 0);
}

/// Frodo claims rewards mid-period (snapshotting his index), then transfers
/// all shares to samwise. Frodo's post-transfer claim is 0; samwise only
/// earns from the transfer point forward. Samwise also inherits the full
/// underlying position and can withdraw it.
#[test]
fn test_claim_before_transfer_preserves_sender_rewards() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let reward_period: u64 = 100_000;
    let f = setup(&e, 100_0000000, 10_000_0000000, reward_period);
    let xlm_client = MockTokenClient::new(&e, &f.xlm);

    e.jump_time(reward_period / 2);

    let frodo_mid_claim = f.fee_vault_client.claim_rewards(&f.frodo, &f.xlm, &f.frodo);
    assert_approx_eq_abs(frodo_mid_claim, 5000_0000000, 1_0000000);

    let frodo_shares = f.fee_vault_client.get_shares(&f.frodo);
    f.fee_vault_client.transfer(&f.frodo, &f.samwise, &frodo_shares);

    e.jump_time(reward_period / 2);

    // frodo holds 0 shares; no new rewards accrue
    let frodo_final_claim = f.fee_vault_client.claim_rewards(&f.frodo, &f.xlm, &f.frodo);
    let samwise_xlm = f.fee_vault_client.claim_rewards(&f.samwise, &f.xlm, &f.samwise);

    assert_eq!(frodo_final_claim, 0);
    assert_approx_eq_abs(samwise_xlm, 5000_0000000, 1_0000000);
    assert_eq!(xlm_client.balance(&f.frodo), frodo_mid_claim);

    // samwise holds all shares and can withdraw the full underlying position
    let samwise_underlying = f.fee_vault_client.get_underlying_tokens(&f.samwise);
    assert!(samwise_underlying > 0);
    f.fee_vault_client.withdraw(&samwise_underlying, &f.samwise, &f.samwise, &f.samwise);
    assert_eq!(f.fee_vault_client.get_shares(&f.samwise), 0);
}

/// Shares pass through three holders in equal thirds: frodo → samwise → merry.
/// Each should earn one third of total rewards. Only the final holder can
/// withdraw the underlying position.
#[test]
fn test_chain_of_transfers() {
    let e = Env::default();
    e.cost_estimate().budget().reset_unlimited();
    e.mock_all_auths();
    e.set_default_info();

    let reward_period: u64 = 90_000;
    let xlm_rewards: i128 = 9_000_0000000;
    let f = setup(&e, 100_0000000, xlm_rewards, reward_period);

    let merry = Address::generate(&e);

    // third 1: frodo holds all
    e.jump_time(reward_period / 3);
    let frodo_shares = f.fee_vault_client.get_shares(&f.frodo);
    f.fee_vault_client.transfer(&f.frodo, &f.samwise, &frodo_shares);

    // third 2: samwise holds all
    e.jump_time(reward_period / 3);
    let samwise_shares = f.fee_vault_client.get_shares(&f.samwise);
    f.fee_vault_client.transfer(&f.samwise, &merry, &samwise_shares);

    // third 3: merry holds all
    e.jump_time(reward_period / 3);

    let expected_per_third = xlm_rewards / 3;

    let frodo_xlm = f.fee_vault_client.claim_rewards(&f.frodo, &f.xlm, &f.frodo);
    let samwise_xlm = f.fee_vault_client.claim_rewards(&f.samwise, &f.xlm, &f.samwise);
    let merry_xlm = f.fee_vault_client.claim_rewards(&merry, &f.xlm, &merry);

    assert_approx_eq_abs(frodo_xlm,   expected_per_third, 1_0000000);
    assert_approx_eq_abs(samwise_xlm, expected_per_third, 1_0000000);
    assert_approx_eq_abs(merry_xlm,   expected_per_third, 1_0000000);
    assert_approx_eq_abs(frodo_xlm + samwise_xlm + merry_xlm, xlm_rewards, 2_0000000);

    // only merry holds shares; she withdraws the full underlying position
    assert_eq!(f.fee_vault_client.get_shares(&f.frodo), 0);
    assert_eq!(f.fee_vault_client.get_shares(&f.samwise), 0);
    let merry_underlying = f.fee_vault_client.get_underlying_tokens(&merry);
    assert!(merry_underlying > 0);
    f.fee_vault_client.withdraw(&merry_underlying, &merry, &merry, &merry);
    assert_eq!(f.fee_vault_client.get_shares(&merry), 0);
}