#![cfg(test)]

use crate::testutils::create_funded_blend_vault;
use crate::BlendVaultClient;
use sep_41_token::testutils::MockTokenClient;
use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
use soroban_sdk::{Address, Env, Error, IntoVal};

const DEPOSIT: i128 = 100_0000000;
/// Far enough ahead that the allowance never expires mid-test.
const EXPIRATION: u32 = 500_000;

/// Builds a vault with a funded `victim` position.
/// Returns (vault address, usdc address, victim, other party).
fn setup(e: &Env) -> (Address, Address, Address, Address) {
    let (vault, usdc) = create_funded_blend_vault(e);

    let victim = Address::generate(e);
    MockTokenClient::new(e, &usdc).mint(&victim, &DEPOSIT);
    BlendVaultClient::new(e, &vault).deposit(&DEPOSIT, &victim, &victim, &victim);

    (vault, usdc, victim, Address::generate(e))
}

/// An operator holding neither the owner's signature nor an allowance from them
/// must not be able to move the owner's position.
#[test]
fn test_withdraw_rejects_unauthorized_operator() {
    let e = Env::default();
    let (vault, usdc, victim, attacker) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);
    let usdc_client = MockTokenClient::new(&e, &usdc);

    let victim_shares = vault_client.get_shares(&victim);
    let victim_value = vault_client.get_underlying_tokens(&victim);
    assert!(victim_shares > 0);
    assert!(victim_value > 0);

    // Only the attacker signs; the victim authorizes nothing and has approved
    // no allowance.
    e.mock_auths(&[MockAuth {
        address: &attacker,
        invoke: &MockAuthInvoke {
            contract: &vault,
            fn_name: "withdraw",
            args: (victim_value, attacker.clone(), victim.clone(), attacker.clone())
                .into_val(&e),
            sub_invokes: &[],
        },
    }]);
    let result = vault_client.try_withdraw(&victim_value, &attacker, &victim, &attacker);

    assert_eq!(result.err(), Some(Ok(Error::from_contract_error(10))));
    assert_eq!(vault_client.get_shares(&victim), victim_shares);
    assert_eq!(usdc_client.balance(&attacker), 0);
}

/// The same attack against `redeem`, which must not inherit `withdraw`'s shape.
#[test]
fn test_redeem_rejects_unauthorized_operator() {
    let e = Env::default();
    let (vault, usdc, victim, attacker) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);
    let usdc_client = MockTokenClient::new(&e, &usdc);

    let victim_shares = vault_client.get_shares(&victim);
    assert!(victim_shares > 0);

    e.mock_auths(&[MockAuth {
        address: &attacker,
        invoke: &MockAuthInvoke {
            contract: &vault,
            fn_name: "redeem",
            args: (victim_shares, attacker.clone(), victim.clone(), attacker.clone())
                .into_val(&e),
            sub_invokes: &[],
        },
    }]);
    let result = vault_client.try_redeem(&victim_shares, &attacker, &victim, &attacker);

    assert_eq!(result.err(), Some(Ok(Error::from_contract_error(10))));
    assert_eq!(vault_client.get_shares(&victim), victim_shares);
    assert_eq!(usdc_client.balance(&attacker), 0);
}

/// A third-party operator delegated via the SEP-41 allowance can redeem, and the
/// allowance is consumed by the shares burnt.
#[test]
fn test_redeem_consumes_operator_allowance() {
    let e = Env::default();
    let (vault, usdc, owner, operator) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);
    let usdc_client = MockTokenClient::new(&e, &usdc);

    let owner_shares = vault_client.get_shares(&owner);
    let redeemed = owner_shares / 4;
    vault_client.approve(&owner, &operator, &owner_shares, &EXPIRATION);

    // The operator alone signs — the allowance stands in for the owner.
    e.mock_auths(&[MockAuth {
        address: &operator,
        invoke: &MockAuthInvoke {
            contract: &vault,
            fn_name: "redeem",
            args: (redeemed, operator.clone(), owner.clone(), operator.clone()).into_val(&e),
            sub_invokes: &[],
        },
    }]);
    let assets = vault_client.redeem(&redeemed, &operator, &owner, &operator);

    assert!(assets > 0);
    assert_eq!(usdc_client.balance(&operator), assets);
    assert_eq!(vault_client.get_shares(&owner), owner_shares - redeemed);
    assert_eq!(
        vault_client.allowance(&owner, &operator),
        owner_shares - redeemed
    );
}

/// An allowance smaller than the shares being burnt is not enough.
#[test]
fn test_redeem_rejects_insufficient_allowance() {
    let e = Env::default();
    let (vault, _usdc, owner, operator) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);

    let owner_shares = vault_client.get_shares(&owner);
    vault_client.approve(&owner, &operator, &(owner_shares - 1), &EXPIRATION);

    e.mock_auths(&[MockAuth {
        address: &operator,
        invoke: &MockAuthInvoke {
            contract: &vault,
            fn_name: "redeem",
            args: (owner_shares, operator.clone(), owner.clone(), operator.clone())
                .into_val(&e),
            sub_invokes: &[],
        },
    }]);
    let result = vault_client.try_redeem(&owner_shares, &operator, &owner, &operator);

    assert_eq!(result.err(), Some(Ok(Error::from_contract_error(10))));
    assert_eq!(vault_client.get_shares(&owner), owner_shares);
}

/// `withdraw` delegated by allowance works and consumes the shares actually
/// burnt, which the asset-denominated path only knows after the fact.
#[test]
fn test_withdraw_consumes_operator_allowance() {
    let e = Env::default();
    let (vault, usdc, owner, operator) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);
    let usdc_client = MockTokenClient::new(&e, &usdc);

    let owner_shares = vault_client.get_shares(&owner);
    let assets = vault_client.get_underlying_tokens(&owner) / 4;
    vault_client.approve(&owner, &operator, &owner_shares, &EXPIRATION);

    e.mock_auths(&[MockAuth {
        address: &operator,
        invoke: &MockAuthInvoke {
            contract: &vault,
            fn_name: "withdraw",
            args: (assets, operator.clone(), owner.clone(), operator.clone()).into_val(&e),
            sub_invokes: &[],
        },
    }]);
    let burnt = vault_client.withdraw(&assets, &operator, &owner, &operator);

    assert_eq!(usdc_client.balance(&operator), assets);
    assert_eq!(vault_client.get_shares(&owner), owner_shares - burnt);
    assert_eq!(vault_client.allowance(&owner, &operator), owner_shares - burnt);
}

/// Self-service is unaffected: owner == operator needs exactly one signature and
/// no allowance. A second `require_auth` on the same address in one frame would
/// be a host error, so the equal case must not add one.
#[test]
fn test_self_redeem_needs_single_auth_and_no_allowance() {
    let e = Env::default();
    let (vault, usdc, owner, _other) = setup(&e);
    let vault_client = BlendVaultClient::new(&e, &vault);
    let usdc_client = MockTokenClient::new(&e, &usdc);

    let owner_shares = vault_client.get_shares(&owner);
    assert_eq!(vault_client.allowance(&owner, &owner), 0);

    // This is exactly how YBC's router calls in: the same address in every role.
    e.mock_auths(&[MockAuth {
        address: &owner,
        invoke: &MockAuthInvoke {
            contract: &vault,
            fn_name: "redeem",
            args: (owner_shares, owner.clone(), owner.clone(), owner.clone()).into_val(&e),
            sub_invokes: &[],
        },
    }]);
    let assets = vault_client.redeem(&owner_shares, &owner, &owner, &owner);

    assert!(assets > 0);
    assert_eq!(usdc_client.balance(&owner), assets);
    assert_eq!(vault_client.get_shares(&owner), 0);
}